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

use quanta_index_contract::{
    BatchIngestMode, MetricsSnapshotV1, SearchCorpusTombstoneScope, TextQuerySyntax,
};
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

#[derive(Clone, Copy)]
enum DeltaShape {
    OneFile,
    Delete,
    Mixed,
}

impl DeltaShape {
    const fn name(self) -> &'static str {
        match self {
            Self::OneFile => "one_file",
            Self::Delete => "delete",
            Self::Mixed => "mixed_replace_delete",
        }
    }
}

#[derive(Debug)]
struct PhaseRead {
    pages: u64,
    rows: u64,
    root_bytes: u64,
    page_bytes: u64,
}

#[expect(
    clippy::print_stdout,
    reason = "the manual QI-BB-006-EVIDENCE line must reach the diagnostic run log"
)]
fn measure_delta(files: usize, shape: DeltaShape) -> TestResult {
    if files < 3 {
        return Err("cost probe requires three distinct files".into());
    }
    let mut rt = E2eRuntime::boot_with_client_request_timeout(Duration::from_secs(300))?
        .with_history_max_bytes(256 * 1024 * 1024);
    rt.start()?;

    let mut base = rt.text_search_corpus_batch(
        "src/file_00000.rs",
        "fn base_00000() { old_only_marker(); }",
    )?;
    let initial_file = base
        .replace_scopes
        .first()
        .ok_or("missing initial coverage")?
        .coverage
        .source
        .file
        .clone();
    let initial_semantic_scopes = base
        .semantic_replace_scopes
        .iter()
        .map(|scope| scope.scope.clone())
        .collect::<Vec<_>>();
    let mut deleted_file = None;
    let mut deleted_semantic_scopes = Vec::new();
    for index in 1..files {
        let path = format!("src/file_{index:05}.rs");
        let content = format!("fn base_{index:05}() {{}}");
        let next = rt.text_search_corpus_batch(&path, &content)?;
        if index == 1 {
            deleted_file = Some(
                next.replace_scopes
                    .first()
                    .ok_or("missing deletion coverage")?
                    .coverage
                    .source
                    .file
                    .clone(),
            );
            deleted_semantic_scopes.extend(
                next.semantic_replace_scopes
                    .iter()
                    .map(|scope| scope.scope.clone()),
            );
        }
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
    if matches!(shape, DeltaShape::Delete) {
        delta.replace_scopes.clear();
        delta.semantic_replace_scopes.clear();
        delta
            .tombstone_scopes
            .push(SearchCorpusTombstoneScope { file: initial_file });
        delta
            .semantic_tombstone_scopes
            .extend(initial_semantic_scopes);
    }
    if matches!(shape, DeltaShape::Mixed) {
        let second = rt.text_search_corpus_batch(
            "src/file_00002.rs",
            "fn replacement_00002() { second_changed_needle(); }",
        )?;
        delta.replace_scopes.extend(second.replace_scopes);
        delta
            .semantic_replace_scopes
            .extend(second.semantic_replace_scopes);
        delta.tombstone_scopes.push(SearchCorpusTombstoneScope {
            file: deleted_file.ok_or("missing deletion target")?,
        });
        delta
            .semantic_tombstone_scopes
            .extend(deleted_semantic_scopes);
    }
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
    let expected_changed = usize::from(!matches!(shape, DeltaShape::Delete));
    if result.candidate_ids.len() != expected_changed {
        return Err(format!(
            "expected {expected_changed} changed chunks, got {} candidates",
            result.candidate_ids.len()
        )
        .into());
    }
    let expected_file_1 = usize::from(!matches!(shape, DeltaShape::Mixed));
    let expected_file_2 = usize::from(!matches!(shape, DeltaShape::Mixed));
    for (query, expected) in [
        ("old_only_marker", 0),
        ("base_00001", expected_file_1),
        ("base_00002", expected_file_2),
        (
            "second_changed_needle",
            usize::from(matches!(shape, DeltaShape::Mixed)),
        ),
        ("base_00003", 1),
    ] {
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
    let mut phase_reads = Vec::new();
    for phase in ["before_intent", "under_lock", "build", "open"] {
        phase_reads.push(PhaseRead {
            pages: counter_delta(
                &before,
                &after,
                &format!("lexical_coverage_{phase}_pages_read_total"),
            )?,
            rows: counter_delta(
                &before,
                &after,
                &format!("lexical_coverage_{phase}_rows_decoded_total"),
            )?,
            root_bytes: counter_delta(
                &before,
                &after,
                &format!("lexical_coverage_{phase}_root_bytes_read_total"),
            )?,
            page_bytes: counter_delta(
                &before,
                &after,
                &format!("lexical_coverage_{phase}_page_bytes_read_total"),
            )?,
        });
    }
    if phase_reads.iter().map(|phase| phase.pages).sum::<u64>() != pages
        || phase_reads.iter().map(|phase| phase.rows).sum::<u64>() != rows
        || phase_reads
            .iter()
            .map(|phase| phase.root_bytes)
            .sum::<u64>()
            != root_bytes
        || phase_reads
            .iter()
            .map(|phase| phase.page_bytes)
            .sum::<u64>()
            != page_bytes
    {
        return Err(format!(
            "coverage phase accounting differs from total: phase_reads={phase_reads:?} total_pages={pages} total_rows={rows} total_root_bytes={root_bytes} total_page_bytes={page_bytes}"
        )
        .into());
    }
    println!(
        "QI-BB-006-EVIDENCE kind=daemon_total_pipeline shape={} files={files} base_ms={base_ms} delta_ms={delta_ms} coverage_pages_read={pages} coverage_rows_decoded={rows} coverage_root_bytes_read={root_bytes} coverage_page_bytes_read={page_bytes} coverage_phase_order=before_intent,under_lock,build,open coverage_phase_reads={phase_reads:?} lexical_seal_bytes_hashed={seal_bytes} base_disk_bytes={base_disk_bytes} delta_disk_bytes={delta_disk_bytes} base_process_rss_bytes={base_process_rss} delta_process_rss_bytes={delta_process_rss} rss_semantics={}",
        shape.name(),
        KernelResidentMemoryProbe::semantics(),
    );
    Ok(())
}

#[test]
#[ignore = "manual full-daemon cost probe; run one case per fresh process"]
fn one_file_delta_over_128_files() -> TestResult {
    measure_delta(128, DeltaShape::OneFile)
}

#[test]
#[ignore = "manual full-daemon cost probe; run one case per fresh process"]
fn one_file_delta_over_512_files() -> TestResult {
    measure_delta(512, DeltaShape::OneFile)
}

#[test]
#[ignore = "manual full-daemon cost probe; run one case per fresh process"]
fn one_file_delta_over_2048_files() -> TestResult {
    measure_delta(2048, DeltaShape::OneFile)
}

#[test]
#[ignore = "manual full-daemon cost probe; run one case per fresh process"]
fn mixed_delta_over_128_files() -> TestResult {
    measure_delta(128, DeltaShape::Mixed)
}

#[test]
#[ignore = "manual full-daemon cost probe; run one case per fresh process"]
fn mixed_delta_over_2048_files() -> TestResult {
    measure_delta(2048, DeltaShape::Mixed)
}

#[test]
#[ignore = "manual full-daemon cost probe; run one case per fresh process"]
fn delete_delta_over_128_files() -> TestResult {
    measure_delta(128, DeltaShape::Delete)
}

#[test]
#[ignore = "manual full-daemon cost probe; run one case per fresh process"]
fn delete_delta_over_2048_files() -> TestResult {
    measure_delta(2048, DeltaShape::Delete)
}
