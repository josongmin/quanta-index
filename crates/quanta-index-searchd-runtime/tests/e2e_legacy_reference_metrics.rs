//! QI-BB-015 exact route and socket metric deltas on a pinned old-Q process.
//! This preserves the Legacy oracle; Native telemetry has a separate owner.

use std::{collections::BTreeMap, error::Error, time::Duration};

use quanta_index_contract::{
    GenerationPin, ManifestGeneration, MetricHistogramV1, MetricsSnapshotV1, RepoId, RevisionId,
};
use quanta_index_core::{LexicalWriterPolicy, count_as_f64};
use quanta_index_sdk::{PublishedBatchEvidence, QuantaIndex, SdkError};
use quanta_index_searchd_harness::legacy_reference::{
    LegacySearchdBinary, SearchdBinaryProcess, legacy_source_batch, publish_legacy_source,
};

use crate::fail_closed_wait::{RealTicker, WaitError, WaitTimeout, wait_for};
use crate::process_envelope_scrape::{self as envelope_scrape, expect_eq, wait_for_scrape_with};

type TestResult<T = ()> = Result<T, Box<dyn Error>>;

const SERVED_QUERIES: u64 = 5;
const SAMPLES_PER_SERVED_LEXICAL_QUERY: u64 = 8;
const SAMPLES_PER_TIMEOUT_ERROR: u64 = 5;

#[derive(Debug)]
struct Scrape {
    snapshot: MetricsSnapshotV1,
}

impl Scrape {
    fn take(client: &QuantaIndex) -> TestResult<Self> {
        Ok(Self {
            snapshot: client.observability().metrics_snapshot()?,
        })
    }

    fn counter_delta(&self, earlier: &Self, name: &str) -> TestResult<u64> {
        let value = |scrape: &Self| {
            scrape
                .snapshot
                .counters
                .iter()
                .find(|counter| counter.name == name)
                .map_or(0, |counter| counter.value)
        };
        let before = value(earlier);
        let now = value(self);
        now.checked_sub(before)
            .ok_or_else(|| format!("counter `{name}` went backwards: {before} -> {now}").into())
    }

    fn counter(&self, name: &str) -> TestResult<u64> {
        self.snapshot
            .counters
            .iter()
            .find(|counter| counter.name == name)
            .map(|counter| counter.value)
            .ok_or_else(|| format!("counter `{name}` is in the scrape").into())
    }

    fn gauge(&self, name: &str) -> TestResult<f64> {
        self.snapshot
            .gauges
            .iter()
            .find(|gauge| gauge.name == name)
            .map(|gauge| gauge.value)
            .ok_or_else(|| format!("gauge `{name}` is in the scrape").into())
    }

    fn histogram(&self, name: &str) -> TestResult<&MetricHistogramV1> {
        self.snapshot
            .histograms
            .iter()
            .find(|histogram| histogram.name == name)
            .ok_or_else(|| format!("histogram `{name}` is in the scrape").into())
    }
}

fn with_reference(test: impl FnOnce(&QuantaIndex) -> TestResult) -> TestResult {
    with_reference_environment(&[], |client, _root| test(client))
}

fn with_reference_environment(
    environment: &[(&str, String)],
    test: impl FnOnce(&QuantaIndex, &std::path::Path) -> TestResult,
) -> TestResult {
    let parent = quanta_index_searchd_harness::private_tempdir()?;
    let state_root = parent.path().join("state");
    let binary = LegacySearchdBinary::from_environment()?;
    let process = if environment.is_empty() {
        binary.start(&state_root, 8)?
    } else {
        let mut command = binary.benchmark_command(&state_root, 8)?;
        let _configured = command.envs(environment.iter().map(|(key, value)| (*key, value)));
        SearchdBinaryProcess::start_with_pinned_command(&state_root, command)?
    };
    let outcome = process
        .connect()
        .and_then(|client| test(&client, &state_root));
    let stopped = process.stop();
    outcome.and(stopped)?;
    binary.verify()
}

/// A complete, sealed Source publication releases the writer before activation.
#[test]
fn local_staging_has_no_daemon_writer_and_sealed_publication_releases_heap() -> TestResult {
    let writer = LexicalWriterPolicy::DEFAULT;
    let environment = [
        (
            "QUANTA_INDEX_LEXICAL_WRITER_ENVELOPE_BYTES",
            writer.envelope_bytes().to_string(),
        ),
        (
            "QUANTA_INDEX_LEXICAL_WRITER_HEAP_BYTES",
            writer.writer_heap_bytes().to_string(),
        ),
        ("QUANTA_INDEX_LEXICAL_WRITER_IDLE_SECS", "2".into()),
    ];
    with_reference_environment(&environment, |client, _root| {
        let repo = RepoId::new("repo-idle")?;
        let revision = RevisionId::new("rev-e2e")?;
        let pin = GenerationPin::new(repo.clone(), revision.clone(), ManifestGeneration::new(1));
        let draft = legacy_source_batch(
            &repo,
            &revision,
            1,
            None,
            &[("src/idle.rs", "idle needle", 1)],
        )?;
        let staged = Scrape::take(client)?;
        expect_eq(
            "local staging opens no daemon writer",
            &staged.gauge("lexical_writers_open")?,
            &0.0,
        )?;
        expect_eq(
            "local staging allocates no writer heap",
            &staged.gauge("lexical_writers_allocated_heap_bytes")?,
            &0.0,
        )?;
        expect_eq(
            "no idle release before publication",
            &staged.counter("lexical_writer_idle_releases_total")?,
            &0,
        )?;
        let outcome = client.search_corpus().publish_outcome(&draft)?;
        if !outcome.receipt.applied
            || !outcome.receipt.sealed
            || outcome.receipt.durable_sequence == 0
        {
            return Err(format!("Source was not durably sealed: {:?}", outcome.receipt).into());
        }
        let published = Scrape::take(client)?;
        expect_eq(
            "sealed publication releases the writer",
            &published.gauge("lexical_writers_open")?,
            &0.0,
        )?;
        expect_eq(
            "sealed publication returns the writer heap",
            &published.gauge("lexical_writers_allocated_heap_bytes")?,
            &0.0,
        )?;
        let _ack = client
            .search_corpus()
            .activate_published(&PublishedBatchEvidence::from(&outcome), None)?;
        let served = client
            .lexical()
            .query()
            .native("needle")
            .pinned(pin.clone())
            .top_k(10)
            .execute()?;
        expect_eq("sealed Source query pin", &served.generation, &pin)?;
        expect_eq("sealed Source has one match", &served.results.len(), &1)
    })
}

fn stable_disk_refresh_window(
    mut read: impl FnMut() -> Result<envelope_scrape::Scrape, Box<dyn Error>>,
    bound: Duration,
    initial: &envelope_scrape::Scrape,
) -> Result<envelope_scrape::Scrape, Box<dyn Error>> {
    let after = |scrape: &envelope_scrape::Scrape| -> Result<u64, Box<dyn Error>> {
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

/// Compare the public gauges to independent inode walks over a quiescent pair.
#[test]
fn the_generation_disk_gauges_match_an_independent_walk_after_a_seal() -> TestResult {
    let tick = Duration::from_millis(50);
    let environment = [(
        "QUANTA_INDEX_MAINTENANCE_TICK_MS",
        tick.as_millis().to_string(),
    )];
    with_reference_environment(&environment, |client, root| {
        let (_head, _receipt) = publish_legacy_source(
            client,
            &RepoId::new("repo-disk")?,
            &RevisionId::new("rev-e2e")?,
            1,
            None,
            &[
                ("src/disk.rs", "disk needle one", 1),
                ("src/disk_two.rs", "disk needle two", 2),
            ],
            None,
        )?;
        let lexical_dir = root.join("indexes/lexical");
        let semantic_dir = root.join("indexes/semantic");
        let lexical = walk_generation_bytes(&lexical_dir)?;
        let semantic = walk_generation_bytes(&semantic_dir)?;
        if lexical == 0 || semantic == 0 {
            return Err(format!(
                "sealed generation has no bytes on a track: lexical={lexical} semantic={semantic}"
            )
            .into());
        }
        let bound = tick
            .saturating_mul(10)
            .saturating_add(Duration::from_secs(5));
        let scrape = || {
            Ok(envelope_scrape::Scrape::from_snapshot(
                client.observability().metrics_snapshot()?,
            ))
        };
        let initial = wait_for_scrape_with(
            scrape,
            bound,
            "the disk gauges matching the independent walk",
            |scrape| {
                scrape
                    .gauge("search_corpus_lexical_generation_disk_bytes")
                    .is_ok_and(|bytes| bytes.to_bits() == count_as_f64(lexical).to_bits())
                    && scrape
                        .gauge("search_corpus_semantic_generation_disk_bytes")
                        .is_ok_and(|bytes| bytes.to_bits() == count_as_f64(semantic).to_bits())
            },
        )?;
        let fresh = stable_disk_refresh_window(scrape, bound, &initial)?;
        expect_eq(
            "lexical disk gauge after a fresh scan",
            &fresh.gauge("search_corpus_lexical_generation_disk_bytes")?,
            &count_as_f64(walk_generation_bytes(&lexical_dir)?),
        )?;
        expect_eq(
            "semantic disk gauge after a fresh scan",
            &fresh.gauge("search_corpus_semantic_generation_disk_bytes")?,
            &count_as_f64(walk_generation_bytes(&semantic_dir)?),
        )
    })
}

/// Each regular-file (device, inode) counts once. Writer locks are transient.
fn walk_generation_bytes(root: &std::path::Path) -> TestResult<u64> {
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
            } else if metadata.is_file()
                && !entry.file_name().to_string_lossy().starts_with(".tantivy")
                && seen.insert((metadata.dev(), metadata.ino()))
            {
                total = total.saturating_add(metadata.len());
            }
        }
    }
    Ok(total)
}

fn with_published_reference(
    test: impl FnOnce(&QuantaIndex, &GenerationPin) -> TestResult,
) -> TestResult {
    with_reference(|client| {
        let pin = publish_reference_fixture(client)?;
        test(client, &pin)
    })
}

fn publish_reference_fixture(client: &QuantaIndex) -> TestResult<GenerationPin> {
    let repo = RepoId::new("repo-e2e")?;
    let revision = RevisionId::new("rev-e2e")?;
    let (_head, _receipt) = publish_legacy_source(
        client,
        &repo,
        &revision,
        1,
        None,
        &[
            ("src/alpha.rs", "needle alpha", 1),
            ("src/beta.rs", "needle beta", 2),
        ],
        None,
    )?;
    let pin = GenerationPin::new(repo, revision, ManifestGeneration::new(1));
    let ready = wait_for(
        &RealTicker::new(),
        Duration::from_secs(20),
        Duration::from_millis(50),
        "pinned old-Q metrics query backend ready",
        || client.observability().process_readiness(),
        |report| report.ready,
        |_| true,
    )?;
    expect_eq("active reference backend", &ready.ready, &true)?;
    Ok(pin)
}

fn expect_empty_boot_inventory(scrape: &Scrape) -> TestResult {
    // The pinned reference starts with a newly allocated, empty state root.
    // These are boot-time counts, so publishing must not rewrite any of them.
    for name in [
        "boot_lexical_sealed_generations",
        "boot_lexical_quarantined_generations",
        "boot_semantic_sealed_generations",
        "boot_semantic_quarantined_generations",
        "boot_lexical_interrupted_reclaims_finished",
        "boot_lexical_interrupted_reclaims_unfinished",
        "boot_semantic_interrupted_reclaims_finished",
        "boot_semantic_interrupted_reclaims_unfinished",
        "boot_active_pairs_validated",
        "boot_half_sealed_pairs",
        "boot_auxiliary_rows_restored",
    ] {
        expect_eq(name, &scrape.gauge(name)?, &0.0)?;
    }
    Ok(())
}

#[test]
fn boot_gauges_match_empty_root_and_writer_envelope_reflects_one_sdk_seal() -> TestResult {
    with_reference(|client| {
        let boot = Scrape::take(client)?;
        expect_eq(
            "boot disk refresh failures",
            &boot.counter("maintenance_disk_refresh_failures_total")?,
            &0,
        )?;
        let _present = boot.gauge("search_corpus_lexical_generation_disk_bytes")?;
        let _present = boot.gauge("search_corpus_semantic_generation_disk_bytes")?;
        expect_empty_boot_inventory(&boot)?;

        let pin = publish_reference_fixture(client)?;
        let warm = client
            .lexical()
            .query()
            .native("needle")
            .pinned(pin)
            .top_k(10)
            .execute()?;
        expect_eq("exactly two fixture files", &warm.results.len(), &2)?;
        let ids: std::collections::BTreeSet<_> = warm
            .results
            .iter()
            .map(|row| row.repo_relative_path.as_str())
            .collect();
        expect_eq(
            "the exact two fixture files are queryable",
            &ids,
            &std::collections::BTreeSet::from(["src/alpha.rs", "src/beta.rs"]),
        )?;

        // The query socket may finish closing after the SDK receives its
        // answer. Observe an idle slot before asserting the socket gauges.
        let scrape = wait_for(
            &RealTicker::new(),
            Duration::from_secs(20),
            Duration::from_millis(50),
            "old-Q socket slots idle after fixture publication",
            || Scrape::take(client),
            |scrape| {
                ["query", "ingest"].iter().all(|plane| {
                    scrape
                        .gauge(&format!("ipc_{plane}_dispatch_in_flight"))
                        .is_ok_and(|value| value == 0.0)
                        && scrape
                            .gauge(&format!("ipc_{plane}_connections_live"))
                            .is_ok_and(|value| value == 0.0)
                }) && scrape
                    .gauge("ipc_control_dispatch_in_flight")
                    .is_ok_and(|value| value == 1.0)
                    && scrape
                        .gauge("ipc_control_connections_live")
                        .is_ok_and(|value| value == 1.0)
            },
            |_| true,
        )
        .map_err(|error| -> Box<dyn Error> {
            match error {
                WaitError::Timeout(timeout) => Box::new(timeout),
                WaitError::Terminal(error) => error,
            }
        })?;
        expect_empty_boot_inventory(&scrape)?;

        expect_eq(
            "seal releases",
            &scrape.counter("lexical_writer_seal_releases_total")?,
            &1,
        )?;
        expect_eq("open writers", &scrape.gauge("lexical_writers_open")?, &0.0)?;
        for (name, expected) in [
            ("lexical_text_authority_rebuilds_total", 1),
            ("lexical_text_authority_incremental_updates_total", 0),
            ("lexical_text_authority_docs_derived_total", 2),
            ("lexical_text_authority_docs_retired_total", 0),
            ("lexical_text_authority_shards_written_total", 1),
            ("lexical_text_authority_shards_inherited_total", 0),
        ] {
            expect_eq(name, &scrape.counter(name)?, &expected)?;
        }
        if scrape.gauge("lexical_writers_max")? < 1.0 {
            return Err("the writer ceiling is at least one".into());
        }
        expect_eq(
            "allocated heap follows open writers",
            &scrape.gauge("lexical_writers_allocated_heap_bytes")?,
            &0.0,
        )?;

        for plane in ["query", "control", "ingest"] {
            for suffix in [
                "connections_refused_total",
                "ingress_admission_refusals_total",
                "requests_overloaded_total",
                "requests_overloaded_repo_total",
            ] {
                let name = format!("ipc_{plane}_{suffix}");
                expect_eq(&name, &scrape.counter(&name)?, &0)?;
            }
            let _present = scrape.counter(&format!("ipc_{plane}_dispatch_queue_wait_total"))?;
            let _present = scrape.counter(&format!("ipc_{plane}_response_bytes_total"))?;
            let expected_live = if plane == "control" { 1.0 } else { 0.0 };
            expect_eq(
                &format!("ipc_{plane}_dispatch_in_flight"),
                &scrape.gauge(&format!("ipc_{plane}_dispatch_in_flight"))?,
                &expected_live,
            )?;
            expect_eq(
                &format!("ipc_{plane}_connections_live"),
                &scrape.gauge(&format!("ipc_{plane}_connections_live"))?,
                &expected_live,
            )?;
        }
        if scrape.counter("ingest_batches_admitted_total")? == 0 {
            return Err("the fixture's ingest batch was admitted".into());
        }
        expect_eq(
            "ingest refusals",
            &scrape.counter("ingest_batches_refused_total")?,
            &0,
        )?;
        expect_eq(
            "dev semantic profile",
            &scrape.gauge("boot_semantic_profile_is_dev")?,
            &1.0,
        )?;
        let _present = scrape.counter("maintenance_ticks_total")?;
        expect_eq(
            "sweep failures",
            &scrape.counter("maintenance_sweep_failures_total")?,
            &0,
        )?;
        let _present = scrape.counter("maintenance_disk_refresh_failures_total")?;
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
    })
}

#[test]
fn route_socket_registry_and_diagnostic_tallies_move_by_exactly_the_traffic_sent() -> TestResult {
    with_published_reference(|client, pin| {
        let query = |text: &str| {
            client
                .lexical()
                .query()
                .native(text)
                .pinned(pin.clone())
                .top_k(10)
                .execute()
        };
        // Absorb first-query readiness before the measured scrape interval.
        let warm = query("needle")?;
        expect_eq("warm-up matches both files", &warm.results.len(), &2)?;
        let first = Scrape::take(client)?;
        for _ in 0..SERVED_QUERIES {
            let served = query("needle")?;
            expect_eq("both fixtures match", &served.results.len(), &2)?;
        }
        let second = Scrape::take(client)?;
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
        let code = match query("timeout:0ms /needle\\b/") {
            Err(SdkError::Remote { code, .. }) => code,
            other => {
                return Err(format!("a zero timeout must fail remotely typed: {other:?}").into());
            }
        };
        expect_eq("typed error code", &code.as_wire_str(), &"QUERY_TIMEOUT")?;
        let third = Scrape::take(client)?;
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
    })
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
    with_published_reference(|client, pin| {
        // Preserve the plain-term readiness probe, which cannot populate
        // the regex cache before its first measured query.
        let _warm = client
            .lexical()
            .query()
            .native("needle")
            .pinned(pin.clone())
            .top_k(10)
            .execute()?;
        let before = Scrape::take(client)?;
        let serve = || -> TestResult {
            let result = client
                .lexical()
                .query()
                .sourcegraph(REGEX)
                .pinned(pin.clone())
                .top_k(10)
                .execute()
                .map_err(|error| format!("the regex query failed: {error:?}"))?;
            if result.results.len() != 1 {
                return Err(format!("only alpha matches: {:?}", result.results).into());
            }
            Ok(())
        };

        serve()?;
        let first = Scrape::take(client)?;
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

        serve()?;
        let repeat = Scrape::take(client)?;
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
    })
}

#[test]
fn stable_disk_window_drains_late_history_and_refuses_a_fresh_failure() -> TestResult {
    let snapshot = |starts, failures| envelope_scrape::Scrape {
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

/// TH-2 adapter proof over the public old-Q metrics API.
#[test]
#[expect(
    clippy::panic_in_result_fn,
    reason = "typed-timeout integration assertions intentionally fail the test while setup uses Result"
)]
fn scrape_wait_never_true_predicate_returns_typed_timeout() -> TestResult {
    with_reference(|client| {
        // Warm the metrics path before starting the bounded wait.
        let _warmed =
            envelope_scrape::Scrape::from_snapshot(client.observability().metrics_snapshot()?);
        let error = wait_for_scrape_with(
            || {
                Ok(envelope_scrape::Scrape::from_snapshot(
                    client.observability().metrics_snapshot()?,
                ))
            },
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
    })
}
