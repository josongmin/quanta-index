//! `scan_vs_index` — exploratory scaling experiment: indexed keyword query vs
//! full-text scan, as a function of corpus size.
//!
//! This is **NOT** part of the RFC-DSL-Benchmarking 3-layer model and its output
//! never feeds the committed baselines. The RFC explicitly forbids reporting
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
//! Every emitted row carries the caller-supplied `--source-fingerprint`. A
//! measurement that cannot name the source it came from is not evidence, so the
//! flag is required rather than defaulted.

use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::Instant;

use anyhow::{Result, anyhow, bail};
use quanta_index_contract::lex::LanguageCode;
use quanta_index_contract::{
    BatchIngestMode, ChunkId, ChunkRecord, LQ_VERSION_TAG, LqExpr, LqLeaf, LqOptions, LqQuery,
    LqSpan, ManifestGeneration, RepoId, RepoRelativePath, RevisionId, SearchCorpusIngestBatch,
    SearchCorpusReplaceScope, SearchScopeKey, SearchScopeSurface,
};
use quanta_index_core::{LexicalIndexOpenPort, SearchCorpusBatchBuildPort};
use quanta_index_lexical::LexicalAdapter;
use sha2::{Digest as _, Sha256};

const NEEDLE: &str = "parity_needle_alpha";
const RECORDS_PER_FILE: usize = 1_000;
const EXPERIMENT_REPO: &str = "exp-repo";
const EXPERIMENT_REVISION: &str = "exp-rev";
const DIGEST_DOMAIN_SCOPE: &str = "quanta-index:scan-experiment:scope:v1";
const DIGEST_DOMAIN_MANIFEST: &str = "quanta-index:scan-experiment:manifest:v1";
const DIGEST_DOMAIN_BATCH: &str = "quanta-index:scan-experiment:batch:v1";

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
    chunks: usize,
    chunk_bytes: usize,
    needle_count: usize,
    samples: usize,
    source_fingerprint: String,
}

/// Nearest-rank p50/p95/p99 (ms) over the collected samples; zeros if empty.
fn percentiles(samples_ms: &[f64]) -> (f64, f64, f64) {
    if samples_ms.is_empty() {
        return (0.0, 0.0, 0.0);
    }
    let mut ordered = samples_ms.to_vec();
    ordered.sort_by(f64::total_cmp);
    (
        nearest_rank(&ordered, 50),
        nearest_rank(&ordered, 95),
        nearest_rank(&ordered, 99),
    )
}

fn nearest_rank(ordered: &[f64], pct: usize) -> f64 {
    let rank = pct.saturating_mul(ordered.len()).div_ceil(100).max(1);
    let idx = rank.min(ordered.len()).saturating_sub(1);
    if let Some(value) = ordered.get(idx) {
        return *value;
    }
    0.0
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

fn parse_string(value: Option<String>, flag: &str) -> Result<String> {
    let Some(raw) = value else {
        bail!("{flag} requires a value");
    };
    if raw.trim().is_empty() {
        bail!("{flag} must not be blank");
    }
    Ok(raw)
}

fn parse_args() -> Result<Args> {
    let mut out_dir: Option<PathBuf> = None;
    let mut index_dir: Option<PathBuf> = None;
    let mut source_fingerprint: Option<String> = None;
    let mut chunks = 10_000_usize;
    let mut chunk_bytes = 512_usize;
    let mut needle_count = 10_usize;
    let mut samples = 50_usize;
    let mut it = std::env::args().skip(1);
    while let Some(flag) = it.next() {
        match flag.as_str() {
            "--out-dir" => out_dir = Some(parse_path(it.next(), "--out-dir")?),
            "--index-dir" => index_dir = Some(parse_path(it.next(), "--index-dir")?),
            "--source-fingerprint" => {
                source_fingerprint = Some(parse_string(it.next(), "--source-fingerprint")?);
            }
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
    let Some(source_fingerprint) = source_fingerprint else {
        bail!("--source-fingerprint is required: an unattributed measurement is not evidence");
    };
    // Default the index beside the corpus, never inside it: `rg`/`grep` would
    // otherwise walk index files and the scan side of the comparison would grow
    // with index size instead of corpus size.
    let index_dir = index_dir.unwrap_or_else(|| sibling_index_dir(&out_dir));
    Ok(Args {
        out_dir,
        index_dir,
        chunks,
        chunk_bytes,
        needle_count: needle_count.min(chunks),
        samples: samples.max(1),
        source_fingerprint,
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

/// Canonical `sha256:`-prefixed digest over length-framed parts.
///
/// Mirrors the framing the contract crate uses for content digests, so the
/// experiment's manifest/scope digests are reproducible for identical
/// parameters instead of being fabricated label strings.
fn framed_digest(domain: &str, parts: &[&[u8]]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(domain.as_bytes());
    hasher.update([0]);
    for part in parts {
        hasher.update(part.len().to_string().as_bytes());
        hasher.update([0]);
        hasher.update(part);
    }
    let digest = hasher.finalize();
    let mut encoded = String::with_capacity(
        "sha256:"
            .len()
            .saturating_add(digest.len().saturating_mul(2)),
    );
    encoded.push_str("sha256:");
    for byte in digest {
        let _written = write!(&mut encoded, "{byte:02x}");
    }
    encoded
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
        chunk_id: ChunkId::new(&format!("c{index}")),
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

fn replace_scope(file: &CorpusFile) -> SearchCorpusReplaceScope {
    SearchCorpusReplaceScope {
        scope: SearchScopeKey {
            doc_surface: SearchScopeSurface::File,
            repo_relative_path: RepoRelativePath::new(&file.repo_relative_path),
        },
        scope_digest: framed_digest(
            DIGEST_DOMAIN_SCOPE,
            &[file.repo_relative_path.as_bytes(), file.body.as_bytes()],
        ),
        chunks: file.chunks.clone(),
        symbols: Vec::new(),
    }
}

/// One sealed `ReplaceGeneration` batch carrying every corpus file as its own
/// file-surface scope.
fn ingest_batch(corpus: &Corpus, generation: ManifestGeneration) -> SearchCorpusIngestBatch {
    let replace_scopes: Vec<SearchCorpusReplaceScope> =
        corpus.files.iter().map(replace_scope).collect();
    let scope_digests: Vec<&[u8]> = replace_scopes
        .iter()
        .map(|scope| scope.scope_digest.as_bytes())
        .collect();
    let manifest_digest = framed_digest(DIGEST_DOMAIN_MANIFEST, &scope_digests);
    let batch_digest = framed_digest(DIGEST_DOMAIN_BATCH, &[manifest_digest.as_bytes()]);
    SearchCorpusIngestBatch {
        repo_id: RepoId::new(EXPERIMENT_REPO),
        revision_id: RevisionId::new(EXPERIMENT_REVISION),
        generation,
        base_generation: None,
        manifest_digest,
        batch_digest,
        mode: BatchIngestMode::ReplaceGeneration,
        bundle_payload: None,
        clear_surfaces: Vec::new(),
        replace_scopes,
        tombstone_scopes: Vec::new(),
        semantic_replace_scopes: Vec::new(),
        semantic_tombstone_scopes: Vec::new(),
        seal: true,
    }
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

/// Recursive on-disk byte total under `root`; the index's physical footprint.
fn directory_bytes(root: &Path) -> Result<u64> {
    let mut total = 0_u64;
    let mut pending = vec![root.to_path_buf()];
    while let Some(dir) = pending.pop() {
        let entries =
            std::fs::read_dir(&dir).map_err(|err| anyhow!("read dir {}: {err}", dir.display()))?;
        for entry in entries {
            let entry = entry.map_err(|err| anyhow!("read dir entry {}: {err}", dir.display()))?;
            let metadata = entry
                .metadata()
                .map_err(|err| anyhow!("stat {}: {err}", entry.path().display()))?;
            if metadata.is_dir() {
                pending.push(entry.path());
            } else {
                total = total.saturating_add(metadata.len());
            }
        }
    }
    Ok(total)
}

struct QueryMeasurement {
    p50_ms: f64,
    p95_ms: f64,
    p99_ms: f64,
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
        let result = searcher.search(&query, 64)?;
        collected.push(elapsed_ms(started));
        hits = result.len();
    }
    let (p50_ms, p95_ms, p99_ms) = percentiles(&collected);
    Ok(QueryMeasurement {
        p50_ms,
        p95_ms,
        p99_ms,
        hits,
    })
}

fn run() -> Result<serde_json::Value> {
    let args = parse_args()?;
    let corpus = generate_corpus(&args)?;
    write_scan_corpus(&args.out_dir, &corpus)?;

    let adapter = LexicalAdapter::with_state_root(args.index_dir.clone());
    let repo = RepoId::new(EXPERIMENT_REPO);
    let revision = RevisionId::new(EXPERIMENT_REVISION);
    let generation = ManifestGeneration::new(1);
    let batch = ingest_batch(&corpus, generation);

    let build_started = Instant::now();
    adapter.build_batch(&batch)?;
    let build_ms = elapsed_ms(build_started);
    let index_bytes = directory_bytes(&args.index_dir)?;

    let searcher = adapter.open(&repo, &revision, generation)?;
    let measurement = measure_query(searcher.as_ref(), args.samples)?;

    Ok(serde_json::json!({
        "source_fingerprint": args.source_fingerprint,
        "chunks": saturating_u64(args.chunks),
        "needle_count": saturating_u64(args.needle_count),
        "corpus_bytes": saturating_u64(corpus.bytes),
        "files": saturating_u64(corpus.files.len()),
        "index_bytes": index_bytes,
        "index_build_ms": build_ms,
        "index_query_p50_ms": measurement.p50_ms,
        "index_query_p95_ms": measurement.p95_ms,
        "index_query_p99_ms": measurement.p99_ms,
        "index_query_samples": saturating_u64(args.samples),
        "index_hits": saturating_u64(measurement.hits),
    }))
}

fn main() -> ExitCode {
    match run() {
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
