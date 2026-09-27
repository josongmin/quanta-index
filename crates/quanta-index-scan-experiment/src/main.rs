//! `scan_vs_index` — exploratory scaling experiment: indexed keyword query vs
//! full-text scan, as a function of corpus size.
//!
//! This is **NOT** part of the JUN-08-001 3-layer model and its output
//! never feeds the committed baselines. The ADR explicitly forbids reporting
//! `dirty:no` / DSL latency against a text-only engine as a benchmark, because a
//! daemon IPC round-trip and a `grep` process answer different questions. This
//! tool exists only to make the *scaling* argument concrete in isolation:
//!
//!   - it builds the lexical index **in-process** (no daemon / no IPC), so the
//!     measured query cost is the index lookup itself, not socket plumbing;
//!   - it writes the corpus bytes to `--out-dir` and the index to a *separate*
//!     `--index-dir`, so the companion `tools/benchmark/run_scan_vs_index.py`
//!     can time `rg` / `grep` over the corpus without also walking index files.
//!
//! The point it demonstrates: the index query cost is ~flat in corpus size
//! while a full scan is linear. Build cost (printed separately) is the index's
//! one-time price, amortized over many queries.
//!
//! Ingest goes through the batch authority surface
//! ([`SearchCorpusBatchBuildPort`]) with `seal: true`, which is the same path
//! the runtime uses. The experiment does not construct legacy channel ops: that
//! path never writes a sealed generation identity, so the produced generation
//! was not openable for query (QI-BB-010).
//!
//! The one JSON document on stdout (and, with `--artifact-out`, on disk) is a
//! `BenchArtifactV1` (QI-BB-010): the exact 40-character head of a clean
//! worktree, the corpus digest over the exact bytes written, the run
//! configuration digest, the host, the process's peak RSS, the build phase and
//! the index's disk amplification. The experiment resolves the head itself; a
//! dirty tree or an unresolvable head is a refusal, never an `unknown` stamp
//! and never a caller-supplied label.

use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::Instant;

use anyhow::{Result, anyhow, bail};
use quanta_index_contract::lex::LanguageCode;
use quanta_index_contract::{
    BatchIngestMode, ChunkId, ChunkRecord, LQ_VERSION_TAG, LqExpr, LqLeaf, LqOptions, LqQuery,
    LqSpan, ManifestGeneration, RepoId, RepoRelativePath, RevisionId, SearchCorpusIngestBatch,
    SearchCorpusReplaceScope, SourceFileCoverage, SourceFileKey, SourceFileRevision,
    SourcePublicationEvent, SymbolCoverage, source_event_payload_sha256,
    source_file_unit_set_sha256,
};
use quanta_index_core::{LexicalIndexOpenPort, RequestBudgetV1, SearchCorpusBatchBuildPort};
use quanta_index_ipc::stamp_batch_digest_v1;
use quanta_index_lexical::LexicalAdapter;
use quanta_index_searchd_harness::artifact::{
    BenchArtifactV1, BenchMode, BenchProvenanceV1, BenchRowV1, BenchSyntax, DiskAmplificationV1,
    GitHeadV1, HostV1, LatencySummary, PhaseDurationsV1, ResourceUsageV1, ResultShape, RouteFamily,
    config_digest, corpus_digest, directory_bytes, framed_digest,
};
use sha2::{Digest as _, Sha256};

const DIMENSION: &str = "scan-vs-index";

const NEEDLE: &str = "parity_needle_alpha";
const RECORDS_PER_FILE: usize = 1_000;
const EXPERIMENT_REPO: &str = "exp-repo";
const EXPERIMENT_REVISION: &str = "exp-rev";
const DIGEST_DOMAIN_SCOPE: &str = "quanta-index:scan-experiment:scope:v1";
const DIGEST_DOMAIN_MANIFEST: &str = "quanta-index:scan-experiment:manifest:v1";

#[expect(
    clippy::print_stdout,
    reason = "the experiment emits its one JSON result row on stdout by design"
)]
fn emit_stdout(value: &serde_json::Value) {
    println!("{value}");
}

#[expect(
    clippy::print_stderr,
    reason = "the experiment reports operator errors on stderr by design"
)]
fn emit_stderr(message: &str) {
    eprintln!("{message}");
}

struct Args {
    out_dir: PathBuf,
    index_dir: PathBuf,
    /// Where to also write the artifact; stdout always carries it.
    artifact_out: Option<PathBuf>,
    chunks: usize,
    chunk_bytes: usize,
    needle_count: usize,
    samples: usize,
}

fn parse_usize(value: Option<String>, flag: &str) -> Result<usize> {
    let Some(raw) = value else {
        bail!("{flag} requires a value");
    };
    raw.parse::<usize>()
        .map_err(|err| anyhow!("{flag}: invalid integer `{raw}`: {err}"))
}

fn parse_path(value: Option<String>, flag: &str) -> Result<PathBuf> {
    let Some(raw) = value else {
        bail!("{flag} requires a value");
    };
    Ok(PathBuf::from(raw))
}

fn parse_args() -> Result<Args> {
    let mut out_dir: Option<PathBuf> = None;
    let mut index_dir: Option<PathBuf> = None;
    let mut artifact_out: Option<PathBuf> = None;
    let mut chunks = 10_000_usize;
    let mut chunk_bytes = 512_usize;
    let mut needle_count = 10_usize;
    let mut samples = 50_usize;
    let mut it = std::env::args().skip(1);
    while let Some(flag) = it.next() {
        match flag.as_str() {
            "--out-dir" => out_dir = Some(parse_path(it.next(), "--out-dir")?),
            "--index-dir" => index_dir = Some(parse_path(it.next(), "--index-dir")?),
            "--artifact-out" => artifact_out = Some(parse_path(it.next(), "--artifact-out")?),
            "--chunks" => chunks = parse_usize(it.next(), "--chunks")?,
            "--chunk-bytes" => chunk_bytes = parse_usize(it.next(), "--chunk-bytes")?,
            "--needle-count" => needle_count = parse_usize(it.next(), "--needle-count")?,
            "--samples" => samples = parse_usize(it.next(), "--samples")?,
            other => bail!("unknown argument `{other}`"),
        }
    }
    let Some(out_dir) = out_dir else {
        bail!("--out-dir is required");
    };
    if chunks == 0 || chunk_bytes == 0 || samples == 0 {
        bail!("--chunks, --chunk-bytes and --samples must be at least 1");
    }
    if needle_count > chunks {
        bail!("--needle-count {needle_count} exceeds --chunks {chunks}");
    }
    // Default the index beside the corpus, never inside it: `rg`/`grep` would
    // otherwise walk index files and the scan side of the comparison would grow
    // with index size instead of corpus size.
    let index_dir = index_dir.unwrap_or_else(|| sibling_index_dir(&out_dir));
    Ok(Args {
        out_dir,
        index_dir,
        artifact_out,
        chunks,
        chunk_bytes,
        needle_count,
        samples,
    })
}

fn sibling_index_dir(out_dir: &Path) -> PathBuf {
    let mut name = out_dir.file_name().map_or_else(
        || "scan-experiment".to_string(),
        |raw| raw.to_string_lossy().into_owned(),
    );
    name.push_str(".index");
    out_dir
        .parent()
        .map_or_else(|| PathBuf::from(&name), |parent| parent.join(&name))
}

fn saturating_u64(n: usize) -> u64 {
    if let Ok(value) = u64::try_from(n) {
        return value;
    }
    u64::MAX
}

fn saturating_u32(n: usize) -> u32 {
    if let Ok(value) = u32::try_from(n) {
        return value;
    }
    u32::MAX
}

fn elapsed_ms(started: Instant) -> f64 {
    started.elapsed().as_secs_f64() * 1000.0
}

/// One deterministic corpus record of roughly `target_bytes`, optionally seeded
/// with the needle so the keyword query and the scan both have something to find.
fn chunk_text(target_bytes: usize, needle: bool) -> String {
    let mut text = String::with_capacity(target_bytes.saturating_add(48));
    if needle {
        text.push_str(NEEDLE);
        text.push(' ');
    }
    text.push_str("fn filler() { let scope = quartz_token; ");
    while text.len() < target_bytes {
        text.push_str("lorem ipsum dolor sit amet ");
    }
    text.push('}');
    text
}

/// One corpus file: the scan side sees `body`, the index side sees `chunks`.
///
/// The two are generated from the same records so both engines answer the same
/// question over the same bytes.
struct CorpusFile {
    repo_relative_path: String,
    scan_file_name: String,
    body: String,
    chunks: Vec<ChunkRecord>,
}

struct Corpus {
    files: Vec<CorpusFile>,
    bytes: usize,
}

fn chunk_record(
    index: usize,
    repo_relative_path: &str,
    text: &str,
    language: &LanguageCode,
) -> ChunkRecord {
    ChunkRecord {
        chunk_id: ChunkId::new(format!("c{index}")),
        repo_relative_path: RepoRelativePath::new(repo_relative_path),
        language: language.clone(),
        start_byte: 0,
        end_byte: saturating_u32(text.len()),
        start_line: 0,
        end_line: 0,
        text: text.to_string().into_boxed_str(),
        structural: None,
        parent_chunk_id: None,
        source_repo_id: None,
    }
}

fn new_corpus_file(file_index: usize) -> CorpusFile {
    CorpusFile {
        repo_relative_path: format!("src/part-{file_index:06}.rs"),
        scan_file_name: format!("part-{file_index:06}.txt"),
        body: String::new(),
        chunks: Vec::new(),
    }
}

fn generate_corpus(args: &Args) -> Result<Corpus> {
    let language = LanguageCode::new("rust").map_err(|err| anyhow!("language code: {err}"))?;
    let mut files: Vec<CorpusFile> = Vec::new();
    let mut current = new_corpus_file(0);
    let mut corpus_bytes = 0_usize;
    for index in 0..args.chunks {
        if current.chunks.len() >= RECORDS_PER_FILE {
            files.push(current);
            current = new_corpus_file(files.len());
        }
        let text = chunk_text(args.chunk_bytes, index < args.needle_count);
        corpus_bytes = corpus_bytes.saturating_add(text.len()).saturating_add(1);
        current.chunks.push(chunk_record(
            index,
            &current.repo_relative_path,
            &text,
            &language,
        ));
        current.body.push_str(&text);
        current.body.push('\n');
    }
    if !current.chunks.is_empty() {
        files.push(current);
    }
    Ok(Corpus {
        files,
        bytes: corpus_bytes,
    })
}

fn write_scan_corpus(out_dir: &Path, corpus: &Corpus) -> Result<()> {
    std::fs::create_dir_all(out_dir)
        .map_err(|err| anyhow!("create {}: {err}", out_dir.display()))?;
    for file in &corpus.files {
        let path = out_dir.join(&file.scan_file_name);
        std::fs::write(&path, &file.body)
            .map_err(|err| anyhow!("write {}: {err}", path.display()))?;
    }
    Ok(())
}

fn replace_scope(
    file: &CorpusFile,
    repo_id: &RepoId,
    revision_id: &RevisionId,
    language: &LanguageCode,
) -> Result<SearchCorpusReplaceScope> {
    Ok(SearchCorpusReplaceScope {
        coverage: SourceFileCoverage {
            source: SourceFileRevision {
                file: SourceFileKey {
                    source_repo_id: repo_id.clone(),
                    repo_relative_path: RepoRelativePath::new(&file.repo_relative_path),
                },
                revision_id: revision_id.clone(),
                source_sha256: Sha256::digest(file.body.as_bytes()).into(),
            },
            language: language.clone(),
            producer_policy_sha256: Sha256::digest(b"scan-experiment-synthetic-source-v1").into(),
            unit_set_sha256: source_file_unit_set_sha256(&file.chunks, &[])?,
            text_admitted: !file.chunks.is_empty(),
            symbols: SymbolCoverage::NotRequested,
        },
        chunks: file.chunks.clone(),
        symbols: Vec::new(),
    })
}

/// One sealed `ReplaceGeneration` batch carrying every corpus file as its own
/// file-surface scope.
fn ingest_batch(
    corpus: &Corpus,
    generation: ManifestGeneration,
) -> Result<SearchCorpusIngestBatch> {
    let repo_id = RepoId::new(EXPERIMENT_REPO)?;
    let revision_id = RevisionId::new(EXPERIMENT_REVISION)?;
    let language = LanguageCode::new("rust").map_err(|error| anyhow!("language code: {error}"))?;
    let replace_scopes: Vec<SearchCorpusReplaceScope> = corpus
        .files
        .iter()
        .map(|file| replace_scope(file, &repo_id, &revision_id, &language))
        .collect::<Result<_>>()?;
    let scope_digests: Vec<String> = corpus
        .files
        .iter()
        .map(|file| {
            framed_digest(
                DIGEST_DOMAIN_SCOPE,
                &[file.repo_relative_path.as_bytes(), file.body.as_bytes()],
            )
        })
        .collect();
    let digest_refs: Vec<&[u8]> = scope_digests
        .iter()
        .map(|digest| digest.as_bytes())
        .collect();
    let manifest_digest = framed_digest(DIGEST_DOMAIN_MANIFEST, &digest_refs);
    let mut batch = SearchCorpusIngestBatch {
        source_event: SourcePublicationEvent {
            stream_id: "scan-experiment-source-v1".to_string(),
            event_id: format!("scan-experiment-g{}-{manifest_digest}", generation.get()),
            expected_base_event_id: None,
            payload_sha256: [0; 32],
        },
        repo_id,
        revision_id,
        generation,
        base_generation: None,
        manifest_digest,
        batch_digest: String::new(),
        mode: BatchIngestMode::ReplaceGeneration,
        bundle_payload: None,
        clear_surfaces: Vec::new(),
        replace_scopes,
        tombstone_scopes: Vec::new(),
        semantic_replace_scopes: Vec::new(),
        semantic_tombstone_scopes: Vec::new(),
        seal: true,
    };
    batch.source_event.payload_sha256 = source_event_payload_sha256(&batch)?;
    stamp_batch_digest_v1(&mut batch)?;
    batch.validate_v1()?;
    batch.validate_surface_mutations_v1()?;
    Ok(batch)
}

fn make_query() -> LqQuery {
    LqQuery {
        lq_version: LQ_VERSION_TAG,
        expr: LqExpr::Leaf(LqLeaf::Keyword(NEEDLE.to_string())),
        filters: Vec::new(),
        options: LqOptions::defaults(),
        directives: Vec::new(),
        source_span: LqSpan::eof(0),
    }
}

struct QueryMeasurement {
    latency: LatencySummary,
    hits: usize,
}

fn measure_query(
    searcher: &dyn quanta_index_core::LexicalSearcher,
    samples: usize,
) -> Result<QueryMeasurement> {
    let query = make_query();
    let mut collected = Vec::with_capacity(samples);
    let mut hits = 0_usize;
    for _ in 0..samples {
        let started = Instant::now();
        let result = searcher.search(&query, 64, &RequestBudgetV1::unbounded())?;
        collected.push(elapsed_ms(started));
        hits = result.len();
    }
    let latency = LatencySummary::from_samples_ms(&collected)
        .ok_or_else(|| anyhow!("no query samples were collected"))?;
    Ok(QueryMeasurement { latency, hits })
}

/// The experiment's artifact: the in-process index query as its one row,
/// the build as its build phase, and the index bytes over the corpus bytes
/// as its disk amplification.
fn artifact(
    args: &Args,
    corpus: &Corpus,
    git_head: GitHeadV1,
    host: HostV1,
    build_ms: f64,
    index_bytes: u64,
    measurement: &QueryMeasurement,
) -> Result<BenchArtifactV1> {
    let corpus_files: Vec<(String, String)> = corpus
        .files
        .iter()
        .map(|file| (file.scan_file_name.clone(), file.body.clone()))
        .collect();
    let corpus_bytes = saturating_u64(corpus.bytes);
    Ok(BenchArtifactV1 {
        dimension: DIMENSION.to_string(),
        mode: BenchMode::Warm,
        concurrency: 1,
        provenance: BenchProvenanceV1 {
            git_head,
            corpus_digest: corpus_digest(DIMENSION, &corpus_files),
            config_digest: config_digest(
                DIMENSION,
                &[
                    ("chunks", args.chunks.to_string()),
                    ("chunk_bytes", args.chunk_bytes.to_string()),
                    ("needle_count", args.needle_count.to_string()),
                    ("samples", args.samples.to_string()),
                    ("records_per_file", RECORDS_PER_FILE.to_string()),
                    ("top_k", "64".to_string()),
                ],
            ),
            // The experiment drives the lexical adapter alone: no embedding
            // model is exercised.
            model_revision: None,
        },
        host,
        resources: ResourceUsageV1::observe_self()?,
        phases: PhaseDurationsV1 {
            build_ms: Some(build_ms),
            update_ms: None,
            gc_ms: None,
        },
        disk_amplification: Some(DiskAmplificationV1 {
            bytes_written: index_bytes,
            changed_bytes: corpus_bytes,
        }),
        rows: vec![BenchRowV1 {
            scenario_id: format!("scan-vs-index.chunks{}.index_query", args.chunks),
            route_family: RouteFamily::Lexical,
            syntax: BenchSyntax::Native,
            result_shape: if measurement.hits == 0 {
                ResultShape::Empty
            } else {
                ResultShape::Candidates
            },
            latency: Some(measurement.latency),
            qps: None,
            error_count: 0,
            timeout_count: 0,
            result_count: Some(saturating_u64(measurement.hits)),
            typed_error_code: None,
            engine_touched: vec!["lexical-adapter-in-process".to_string()],
            early_stop_reason: None,
        }],
        detail: serde_json::json!({
            "needle": NEEDLE,
            "chunks": saturating_u64(args.chunks),
            "needle_count": saturating_u64(args.needle_count),
            "corpus_bytes": corpus_bytes,
            "files": saturating_u64(corpus.files.len()),
            "index_bytes": index_bytes,
            "index_build_ms": build_ms,
            "index_query_samples": saturating_u64(args.samples),
            "index_hits": saturating_u64(measurement.hits),
            "scan_corpus_dir": args.out_dir.display().to_string(),
        }),
    })
}

fn run() -> Result<BenchArtifactV1> {
    let args = parse_args()?;
    // Provenance first: a run that cannot be attributed is not started.
    let git_head = GitHeadV1::resolve(Path::new("."))?;
    let host = HostV1::observe()?;
    let corpus = generate_corpus(&args)?;
    write_scan_corpus(&args.out_dir, &corpus)?;

    let adapter = LexicalAdapter::with_state_root(args.index_dir.clone());
    let repo = RepoId::new(EXPERIMENT_REPO)?;
    let revision = RevisionId::new(EXPERIMENT_REVISION)?;
    let generation = ManifestGeneration::new(1);
    let batch = ingest_batch(&corpus, generation)?;

    let build_started = Instant::now();
    adapter.build_batch(&batch)?;
    let build_ms = elapsed_ms(build_started);
    let index_bytes = directory_bytes(&args.index_dir)?;

    let searcher = adapter.open(&repo, &revision, generation)?;
    let measurement = measure_query(searcher.as_ref(), args.samples)?;

    let artifact = artifact(
        &args,
        &corpus,
        git_head,
        host,
        build_ms,
        index_bytes,
        &measurement,
    )?;
    if let Some(out) = &args.artifact_out {
        artifact.write_to(out)?;
    }
    Ok(artifact)
}

fn main() -> ExitCode {
    match run().and_then(|artifact| artifact.to_json().map_err(anyhow::Error::from)) {
        Ok(value) => {
            emit_stdout(&value);
            ExitCode::SUCCESS
        }
        Err(err) => {
            emit_stderr(&format!("scan_vs_index: {err:#}"));
            ExitCode::FAILURE
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use quanta_index_ipc::BatchDigestVerdictV1;

    #[test]
    fn current_source_batch_builds_and_answers_an_indexed_hit() -> Result<()> {
        let language = LanguageCode::new("rust").map_err(|error| anyhow!("language: {error}"))?;
        let text = chunk_text(96, true);
        let path = "src/part-000000.rs";
        let body = format!("{text}\n");
        let corpus = Corpus {
            bytes: body.len(),
            files: vec![CorpusFile {
                repo_relative_path: path.to_string(),
                scan_file_name: "part-000000.txt".to_string(),
                body: body.clone(),
                chunks: vec![chunk_record(0, path, &text, &language)],
            }],
        };
        let generation = ManifestGeneration::new(1);
        let batch = ingest_batch(&corpus, generation)?;
        assert_eq!(
            batch.replace_scopes[0].coverage.source.source_sha256,
            Sha256::digest(body.as_bytes()).into()
        );
        assert!(matches!(
            quanta_index_ipc::verify_batch_digest_v1(&mut batch.clone())?,
            BatchDigestVerdictV1::Verified(_)
        ));

        let root = tempfile::tempdir()?;
        let adapter = LexicalAdapter::with_state_root(root.path().join("index"));
        adapter.build_batch(&batch)?;
        let searcher = adapter.open(&batch.repo_id, &batch.revision_id, generation)?;
        assert_eq!(measure_query(searcher.as_ref(), 1)?.hits, 1);
        Ok(())
    }
}
