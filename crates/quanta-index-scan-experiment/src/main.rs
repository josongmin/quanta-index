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
//!   - it writes the identical corpus bytes to `--out-dir` so the companion
//!     `tools/benchmark/run_scan_vs_index.py` can time `rg` / `grep` over the
//!     same data and chart the crossover.
//!
//! The point it demonstrates: the index query cost is ~flat in corpus size
//! while a full scan is linear. Build cost (printed separately) is the index's
//! one-time price, amortized over many queries.

use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::Instant;

use anyhow::{Result, anyhow, bail};
use quanta_index_contract::channel::LexicalChannelOp;
use quanta_index_contract::lex::LanguageCode;
use quanta_index_contract::{
    ChunkId, ChunkRecord, LQ_VERSION_TAG, LqExpr, LqLeaf, LqOptions, LqQuery, LqSpan,
    ManifestGeneration, RepoId, RepoRelativePath, RevisionId, UpsertChunk,
};
use quanta_index_core::{LexicalIndexBuildPort, LexicalIndexOpenPort};
use quanta_index_lexical::LexicalAdapter;

const NEEDLE: &str = "parity_needle_alpha";
const RECORDS_PER_FILE: usize = 1_000;

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
    chunks: usize,
    chunk_bytes: usize,
    needle_count: usize,
    samples: usize,
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

fn parse_args() -> Result<Args> {
    let mut out_dir: Option<PathBuf> = None;
    let mut chunks = 10_000_usize;
    let mut chunk_bytes = 512_usize;
    let mut needle_count = 10_usize;
    let mut samples = 50_usize;
    let mut it = std::env::args().skip(1);
    while let Some(flag) = it.next() {
        match flag.as_str() {
            "--out-dir" => {
                let Some(raw) = it.next() else {
                    bail!("--out-dir requires a value");
                };
                out_dir = Some(PathBuf::from(raw));
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
    Ok(Args {
        out_dir,
        chunks,
        chunk_bytes,
        needle_count: needle_count.min(chunks),
        samples: samples.max(1),
    })
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

fn build_op(index: usize, text: &str) -> Result<LexicalChannelOp> {
    let chunk_id = format!("c{index}");
    let end_byte = saturating_u32(text.len());
    let record = ChunkRecord {
        chunk_id: ChunkId::new(&chunk_id),
        repo_relative_path: RepoRelativePath::new("src/corpus.rs"),
        language: LanguageCode::new("rust").map_err(|err| anyhow!("language code: {err}"))?,
        start_byte: 0,
        end_byte,
        start_line: 0,
        end_line: 0,
        text: text.to_string().into_boxed_str(),
        structural: None,
        parent_chunk_id: None,
        source_repo_id: None,
    };
    let mut payload = Vec::new();
    ciborium::into_writer(&record, &mut payload).map_err(|err| anyhow!("encode chunk: {err}"))?;
    Ok(LexicalChannelOp::UpsertChunk(UpsertChunk {
        repo_id: RepoId::new("exp-repo"),
        revision_id: RevisionId::new("exp-rev"),
        generation: ManifestGeneration::new(1),
        chunk_id: ChunkId::new(&chunk_id),
        payload,
    }))
}

fn write_part(out_dir: &Path, file_index: usize, body: &str) -> Result<()> {
    let path = out_dir.join(format!("part-{file_index:06}.txt"));
    std::fs::write(&path, body).map_err(|err| anyhow!("write {}: {err}", path.display()))
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

struct Corpus {
    ops: Vec<LexicalChannelOp>,
    bytes: usize,
    files: usize,
}

fn generate_corpus(args: &Args) -> Result<Corpus> {
    std::fs::create_dir_all(&args.out_dir)
        .map_err(|err| anyhow!("create {}: {err}", args.out_dir.display()))?;
    let mut ops = Vec::with_capacity(args.chunks);
    let mut body = String::new();
    let mut in_file = 0_usize;
    let mut files = 0_usize;
    let mut corpus_bytes = 0_usize;
    for index in 0..args.chunks {
        let text = chunk_text(args.chunk_bytes, index < args.needle_count);
        corpus_bytes = corpus_bytes.saturating_add(text.len()).saturating_add(1);
        ops.push(build_op(index, &text)?);
        body.push_str(&text);
        body.push('\n');
        in_file = in_file.saturating_add(1);
        if in_file >= RECORDS_PER_FILE {
            write_part(&args.out_dir, files, &body)?;
            body.clear();
            in_file = 0;
            files = files.saturating_add(1);
        }
    }
    if !body.is_empty() {
        write_part(&args.out_dir, files, &body)?;
        files = files.saturating_add(1);
    }
    Ok(Corpus {
        ops,
        bytes: corpus_bytes,
        files,
    })
}

fn run() -> Result<serde_json::Value> {
    let args = parse_args()?;
    let corpus = generate_corpus(&args)?;

    let adapter = LexicalAdapter::with_state_root(args.out_dir.join(".index"));
    let repo = RepoId::new("exp-repo");
    let revision = RevisionId::new("exp-rev");
    let generation = ManifestGeneration::new(1);

    let build_started = Instant::now();
    adapter.build(&repo, &revision, generation, &corpus.ops)?;
    let build_ms = elapsed_ms(build_started);

    let searcher = adapter.open(&repo, &revision, generation)?;
    let query = make_query();
    let mut samples = Vec::with_capacity(args.samples);
    let mut hits = 0_usize;
    for _ in 0..args.samples {
        let started = Instant::now();
        let result = searcher.search(&query, 64)?;
        samples.push(elapsed_ms(started));
        hits = result.len();
    }
    let (p50, p95, p99) = percentiles(&samples);

    Ok(serde_json::json!({
        "chunks": saturating_u64(args.chunks),
        "needle_count": saturating_u64(args.needle_count),
        "corpus_bytes": saturating_u64(corpus.bytes),
        "files": saturating_u64(corpus.files),
        "index_build_ms": build_ms,
        "index_query_p50_ms": p50,
        "index_query_p95_ms": p95,
        "index_query_p99_ms": p99,
        "index_query_samples": saturating_u64(args.samples),
        "index_hits": saturating_u64(hits),
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
