//! Manual, full-daemon coverage-delta cost probe.
//!
//! The lexical adapter probe accounts for its own reads. This probe also
//! includes semantic publication, sealing, catalog writes and the IPC path.
//! Run one ignored case per fresh pinned daemon. The base is built in that
//! child, so the daemon RSS observation includes its base high-water mark;
//! it is not delta-only allocator evidence. SDK IPC is included in timing.

#![forbid(unsafe_code)]

use std::error::Error;
use std::time::{Duration, Instant};

use quanta_index_contract::lex::LanguageCode;
use quanta_index_contract::{
    ChunkId, ChunkRecord, GenerationPin, ManifestGeneration, MetricsSnapshotV1,
    QueryConstraintSetV1, RepoId, RepoRelativePath, RevisionId, SourceFileKey,
    SourcePublicationEvent, TextQueryRequest, TextQuerySyntax,
};
use quanta_index_core::domains::generation::unique_inode_tree_bytes;
use quanta_index_sdk::{BatchMode, ConnectOptions, PublishedBatchEvidence, QuantaIndex};
use quanta_index_searchd::app::KernelResidentMemoryProbe;
use quanta_index_searchd_harness::legacy_reference::{
    LegacySearchdBinary, SearchdBinaryProcess, legacy_source_delta_from_chunks,
    legacy_source_replacement_from_chunks, request_when_ready,
};
use quanta_index_searchd_harness::private_tempdir;
use quanta_index_searchd_harness::scale_runtime_policy::DEFAULT_HISTORY_MAX_GENERATIONS;

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

fn regular_file_bytes(root: &std::path::Path) -> Result<u64, Box<dyn Error>> {
    Ok(unique_inode_tree_bytes(
        &[root.to_path_buf()],
        &|_| false,
        None,
    )?)
}

fn daemon_rss(snapshot: &MetricsSnapshotV1) -> Result<f64, Box<dyn Error>> {
    // The production gauge calls the same KernelResidentMemoryProbe as the
    // old in-process test. Its process is now the pinned child daemon.
    let value = snapshot
        .gauges
        .iter()
        .find(|point| point.name == "process_resident_bytes")
        .ok_or("missing child daemon resident-memory gauge")?
        .value;
    if !value.is_finite() || value <= 0.0 {
        return Err("invalid child daemon resident-memory gauge".into());
    }
    Ok(value)
}

fn source_chunk(path: &str, content: String) -> Result<ChunkRecord, Box<dyn Error>> {
    Ok(ChunkRecord {
        chunk_id: ChunkId::new(format!("e2e-idem-{path}")),
        repo_relative_path: RepoRelativePath::new(path),
        language: LanguageCode::new("rust").map_err(std::io::Error::other)?,
        start_byte: 0,
        end_byte: u32::try_from(content.len())?,
        start_line: 1,
        end_line: 2,
        text: content.into_boxed_str(),
        structural: None,
        parent_chunk_id: None,
        source_repo_id: None,
    })
}

fn source_event(sequence: usize, previous: Option<usize>) -> SourcePublicationEvent {
    SourcePublicationEvent {
        stream_id: "e2e-harness-source-v1".into(),
        event_id: format!("e2e-source-event-{sequence}"),
        expected_base_event_id: previous.map(|value| format!("e2e-source-event-{value}")),
        payload_sha256: [0; 32],
    }
}

fn query_count(
    client: &QuantaIndex,
    pin: &GenerationPin,
    query: &str,
) -> Result<usize, Box<dyn Error>> {
    // The original harness sends this exact Native DTO and retries NotReady.
    let request = TextQueryRequest {
        syntax: TextQuerySyntax::Native,
        query_text: query.to_owned(),
        constraints: QueryConstraintSetV1::unconstrained(),
        generation: Some(pin.clone()),
        generation_selector: None,
        top_k: 5,
        cursor: None,
    };
    Ok(
        request_when_ready(|| client.lexical().query_request(request.clone()))?
            .results
            .len(),
    )
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

fn measure_delta(files: usize, shape: DeltaShape) -> TestResult {
    if files < 4 {
        return Err("cost probe requires three edit candidates and one unaffected file".into());
    }
    let root = private_tempdir()?;
    let state_root = std::fs::canonicalize(root.path())?;
    let binary = LegacySearchdBinary::from_environment()?;
    let mut command = binary.benchmark_command(&state_root, DEFAULT_HISTORY_MAX_GENERATIONS)?;
    // The original E2eRuntime widened the per-pair history bound to 256 MiB.
    let _configured = command.env(
        "QUANTA_INDEX_SEARCH_CORPUS_HISTORY_MAX_BYTES",
        (256 * 1024 * 1024).to_string(),
    );
    let process = SearchdBinaryProcess::start_with_pinned_command_and_timeout(
        &state_root,
        command,
        Duration::from_secs(300),
    )?;
    let result = (|| -> TestResult {
        let client = QuantaIndex::connect_legacy(
            ConnectOptions::from_state_root(state_root.clone())
                .with_request_io_timeout(Duration::from_secs(300)),
        )?;
        measure_delta_with_client(&client, &state_root, files, shape)
    })();
    let stopped = process.stop();
    result?;
    stopped?;
    binary.verify()
}

#[expect(
    clippy::print_stdout,
    reason = "the manual QI-BB-006-EVIDENCE line must reach the diagnostic run log"
)]
fn measure_delta_with_client(
    client: &QuantaIndex,
    state_root: &std::path::Path,
    files: usize,
    shape: DeltaShape,
) -> TestResult {
    let repo = RepoId::new("repo-e2e")?;
    let revision = RevisionId::new("rev-e2e")?;
    let initial_path = "src/file_00000.rs";
    let initial = source_chunk(
        initial_path,
        "fn base_00000() { old_only_marker(); }".into(),
    )?;
    let deleted_path = "src/file_00001.rs";
    let mut base_chunks = vec![initial.clone()];
    for index in 1..files {
        let path = format!("src/file_{index:05}.rs");
        let content = format!("fn base_{index:05}() {{}}");
        base_chunks.push(source_chunk(&path, content)?);
    }
    let deleted = base_chunks.get(1).ok_or("missing deletion target")?.clone();
    // Each old text_search_corpus_batch consumed one event sequence, then
    // issue_fixture_source_event stamped the complete G1 body as files + 1.
    let base_event = files + 1;
    let base = legacy_source_replacement_from_chunks(&repo, &revision, 1, base_chunks)?
        .source_event(source_event(base_event, None));
    let started = Instant::now();
    let base_outcome = client.search_corpus().publish_outcome(&base)?;
    let base_ms = started.elapsed().as_millis();
    if !base_outcome.receipt.applied || base_outcome.receipt.durable_sequence == 0 {
        return Err("G1 Source publication was not durable".into());
    }
    let active_g1 = client
        .search_corpus()
        .activate_published(&PublishedBatchEvidence::from(&base_outcome), None)?
        .active;
    if client
        .generations()
        .active_head(repo.clone(), revision.clone())?
        != Some(active_g1.clone())
    {
        return Err("G1 Source active head differs from activation".into());
    }
    let before = client.observability().metrics_snapshot()?;
    let base_disk_bytes = regular_file_bytes(state_root)?;
    let base_process_rss = daemon_rss(&before)?;

    let updated = source_chunk(initial_path, "fn base_00000() { changed_needle(); }".into())?;
    let mut replacements = if matches!(shape, DeltaShape::Delete) {
        Vec::new()
    } else {
        vec![updated]
    };
    let mut deleted_files = Vec::new();
    let mut retired = Vec::new();
    if matches!(shape, DeltaShape::Delete) {
        deleted_files.push(SourceFileKey {
            source_repo_id: repo.clone(),
            repo_relative_path: initial.repo_relative_path.clone(),
        });
        retired.push(initial);
    }
    if matches!(shape, DeltaShape::Mixed) {
        replacements.push(source_chunk(
            "src/file_00002.rs",
            "fn replacement_00002() { second_changed_needle(); }".into(),
        )?);
        deleted_files.push(SourceFileKey {
            source_repo_id: repo.clone(),
            repo_relative_path: RepoRelativePath::new(deleted_path),
        });
        retired.push(deleted);
    }
    // G2's draft consumes one sequence; Mixed's second draft consumes one
    // more. The final complete Source event is then stamped once.
    let delta_event = files
        + if matches!(shape, DeltaShape::Mixed) {
            4
        } else {
            3
        };
    let delta = legacy_source_delta_from_chunks(
        &repo,
        &revision,
        2,
        &retired,
        &deleted_files,
        replacements,
    )?
    .source_event(source_event(delta_event, Some(base_event)));
    if delta.mode() != BatchMode::Delta
        || delta.base_generation() != Some(ManifestGeneration::new(1))
    {
        return Err("successor must be a delta".into());
    }
    let started = Instant::now();
    let delta_outcome = client.search_corpus().publish_outcome(&delta)?;
    let delta_ms = started.elapsed().as_millis();
    if !delta_outcome.receipt.applied
        || delta_outcome.receipt.durable_sequence <= base_outcome.receipt.durable_sequence
    {
        return Err("G2 Source publication was not a new durable apply".into());
    }
    // Preserve the original boundary: metrics, inode bytes and RSS are read
    // after the G2 seal but before its active-head CAS.
    let after = client.observability().metrics_snapshot()?;
    let delta_disk_bytes = regular_file_bytes(state_root)?;
    let delta_process_rss = daemon_rss(&after)?;

    let active_g2 = client
        .search_corpus()
        .activate_published(
            &PublishedBatchEvidence::from(&delta_outcome),
            Some(active_g1),
        )?
        .active;
    if client
        .generations()
        .active_head(repo.clone(), revision.clone())?
        != Some(active_g2)
    {
        return Err("G2 Source active head differs from activation".into());
    }
    let pin = GenerationPin::new(repo, revision, ManifestGeneration::new(2));
    let expected_changed = usize::from(!matches!(shape, DeltaShape::Delete));
    let changed = query_count(client, &pin, "changed_needle")?;
    if changed != expected_changed {
        return Err(format!(
            "expected {expected_changed} changed chunks, got {changed} candidates",
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
        let count = query_count(client, &pin, query)?;
        if count != expected {
            return Err(format!("{query}: expected {expected} candidates, got {count}").into());
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
        "QI-BB-006-EVIDENCE kind=daemon_total_pipeline shape={} files={files} base_ms={base_ms} delta_ms={delta_ms} coverage_pages_read={pages} coverage_rows_decoded={rows} coverage_root_bytes_read={root_bytes} coverage_page_bytes_read={page_bytes} coverage_phase_order=before_intent,under_lock,build,open coverage_phase_reads={phase_reads:?} lexical_seal_bytes_hashed={seal_bytes} base_disk_bytes={base_disk_bytes} delta_disk_bytes={delta_disk_bytes} base_process_rss_bytes={base_process_rss:.0} delta_process_rss_bytes={delta_process_rss:.0} rss_scope=child_daemon rss_semantics={} timing_scope=external_sdk_publish",
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
