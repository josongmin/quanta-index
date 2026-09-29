//! Manual, full-daemon coverage-delta cost probe.
//!
//! The lexical adapter probe accounts for its own reads. This probe also
//! includes semantic publication, sealing, catalog writes and the IPC path.
//! Run one ignored case per fresh process when comparing process high-water
//! marks. The base is built in that process, so its high-water mark is still
//! part of the reported value; it is not delta-only allocator evidence.

#![forbid(unsafe_code)]

use std::error::Error;
use std::time::{Duration, Instant};

use quanta_index_contract::{BatchIngestMode, MetricsSnapshotV1, TextQuerySyntax};
use quanta_index_core::ProcessMemoryProbePort as _;
use quanta_index_core::domains::generation::unique_inode_tree_bytes;
use quanta_index_searchd::app::KernelResidentMemoryProbe;
use quanta_index_searchd_harness::E2eRuntime;

type TestResult = Result<(), Box<dyn Error>>;

fn counter(snapshot: &MetricsSnapshotV1, name: &str) -> Result<u64, Box<dyn Error>> {
    snapshot
        .counters
        .iter()
        .find(|point| point.name == name)
        .map(|point| point.value)
        .ok_or_else(|| format!("missing daemon counter `{name}`").into())
}

fn counter_delta(
    before: &MetricsSnapshotV1,
    after: &MetricsSnapshotV1,
    name: &str,
) -> Result<u64, Box<dyn Error>> {
    counter(after, name)?
        .checked_sub(counter(before, name)?)
        .ok_or_else(|| format!("daemon counter `{name}` regressed").into())
}

fn regular_file_bytes(rt: &E2eRuntime) -> Result<u64, Box<dyn Error>> {
    Ok(unique_inode_tree_bytes(
        &[rt.state_root().to_path_buf()],
        &|_| false,
    )?)
}

#[expect(
    clippy::print_stdout,
    reason = "the manual QI-BB-006-EVIDENCE line must reach the diagnostic run log"
)]
fn measure_delta(files: usize) -> TestResult {
    let mut rt = E2eRuntime::boot_with_client_request_timeout(Duration::from_secs(300))?
        .with_history_max_bytes(256 * 1024 * 1024);
    rt.start()?;

    let mut base = rt.text_search_corpus_batch(
        "src/file_00000.rs",
        "fn base_00000() { old_only_marker(); }",
    )?;
    for index in 1..files {
        let path = format!("src/file_{index:05}.rs");
        let content = format!("fn base_{index:05}() {{}}");
        let next = rt.text_search_corpus_batch(&path, &content)?;
        base.replace_scopes.extend(next.replace_scopes);
        base.semantic_replace_scopes
            .extend(next.semantic_replace_scopes);
    }
    rt.issue_fixture_source_event(&mut base)?;
    base.validate_v1()?;
    let started = Instant::now();
    rt.publish_search_corpus_batch(base)?;
    let base_ms = started.elapsed().as_millis();
    rt.activate_last_sealed_generation()?;
    let before = rt.metrics_snapshot()?;
    let base_disk_bytes = regular_file_bytes(&rt)?;
    let base_process_rss = KernelResidentMemoryProbe.resident_bytes()?;

    let mut delta =
        rt.text_search_corpus_batch("src/file_00000.rs", "fn base_00000() { changed_needle(); }")?;
    if delta.mode != BatchIngestMode::Delta {
        return Err("successor must be a delta".into());
    }
    rt.issue_fixture_source_event(&mut delta)?;
    delta.validate_v1()?;
    let started = Instant::now();
    rt.publish_search_corpus_batch(delta)?;
    let delta_ms = started.elapsed().as_millis();
    let after = rt.metrics_snapshot()?;
    let delta_disk_bytes = regular_file_bytes(&rt)?;
    let delta_process_rss = KernelResidentMemoryProbe.resident_bytes()?;

    rt.activate_last_sealed_generation()?;
    let result = rt.query_text(TextQuerySyntax::Native, "changed_needle", 5);
    if let Some(error) = result.typed_error {
        return Err(format!("delta query failed: {error}").into());
    }
    if result.candidate_ids.len() != 1 {
        return Err(format!(
            "expected one changed chunk, got {} candidates",
            result.candidate_ids.len()
        )
        .into());
    }
    for (query, expected) in [("old_only_marker", 0), ("base_00001", 1)] {
        let result = rt.query_text(TextQuerySyntax::Native, query, 5);
        if let Some(error) = result.typed_error {
            return Err(format!("{query} query failed: {error}").into());
        }
        if result.candidate_ids.len() != expected {
            return Err(format!(
                "{query}: expected {expected} candidates, got {}",
                result.candidate_ids.len()
            )
            .into());
        }
    }

    let pages = counter_delta(&before, &after, "lexical_coverage_pages_read_total")?;
    let rows = counter_delta(&before, &after, "lexical_coverage_rows_decoded_total")?;
    let root_bytes = counter_delta(&before, &after, "lexical_coverage_root_bytes_read_total")?;
    let page_bytes = counter_delta(&before, &after, "lexical_coverage_page_bytes_read_total")?;
    let seal_bytes = counter_delta(&before, &after, "lexical_seal_bytes_hashed_total")?;
    if pages == 0 || rows == 0 || page_bytes == 0 || seal_bytes == 0 {
        return Err("full delta did not exercise coverage reads and seal".into());
    }
    println!(
        "QI-BB-006-EVIDENCE kind=daemon_total_pipeline files={files} base_ms={base_ms} delta_ms={delta_ms} coverage_pages_read={pages} coverage_rows_decoded={rows} coverage_root_bytes_read={root_bytes} coverage_page_bytes_read={page_bytes} lexical_seal_bytes_hashed={seal_bytes} base_disk_bytes={base_disk_bytes} delta_disk_bytes={delta_disk_bytes} base_process_rss_bytes={base_process_rss} delta_process_rss_bytes={delta_process_rss} rss_semantics={}",
        KernelResidentMemoryProbe::semantics(),
    );
    Ok(())
}

#[test]
#[ignore = "manual full-daemon cost probe; run one case per fresh process"]
fn one_file_delta_over_128_files() -> TestResult {
    measure_delta(128)
}

#[test]
#[ignore = "manual full-daemon cost probe; run one case per fresh process"]
fn one_file_delta_over_512_files() -> TestResult {
    measure_delta(512)
}

#[test]
#[ignore = "manual full-daemon cost probe; run one case per fresh process"]
fn one_file_delta_over_2048_files() -> TestResult {
    measure_delta(2048)
}
