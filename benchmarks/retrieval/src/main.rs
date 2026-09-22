//! `quanta-index-retrieval-bench`: real-repository SDK runner binary.
//!
//! `run` loads an admitted manifest, chunks it, boots a real `searchd`,
//! publishes through the public SDK, queries SDK routes and emits a v2
//! runner record. `chunk` inspects chunking without a daemon. Unknown flags
//! fail; nothing is guessed.

#![forbid(unsafe_code)]

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use quanta_index_retrieval_bench::batch::{BatchIdentity, assemble_batch};
use quanta_index_retrieval_bench::chunking::{
    Chunker, CoverageReport, STRATEGY_FIXED_WINDOW, STRATEGY_SYNTAX, STRATEGY_WHOLE_FILE,
    chunk_corpus, fixed_window::FixedWindowChunker, syntax::SyntaxChunker,
    whole_file::WholeFileChunker,
};
use quanta_index_retrieval_bench::corpus::{CorpusLimits, SourceFile, load_corpus, load_manifest};
use quanta_index_retrieval_bench::record::{
    QueryPack, RouteProvenance, RunnerIdentity, load_query_pack, runner_record,
};
use quanta_index_retrieval_bench::sdk::{
    DEFAULT_IO_TIMEOUT, DEFAULT_READY_TIMEOUT, DaemonConfig, DaemonSession, QueryOutcome,
    RouteQuery, publish_and_activate, query_route,
};
use quanta_index_retrieval_bench::{BenchError, BenchResult, sha256_hex};

const KNOWN_ROUTES: [&str; 3] = ["lexical", "semantic", "hybrid"];

fn usage_error(message: String) -> BenchError {
    BenchError::Config(format!("{message} (see --help)"))
}

fn print_help() {
    println!(
        "quanta-index-retrieval-bench run|chunk [flags]\n\
         \n\
         run: manifest -> chunks -> real searchd publish/activate -> SDK queries -> v2 record\n\
         chunk: manifest -> chunks + coverage JSON (no daemon)\n\
         \n\
         shared: --repo PATH --manifest PATH --strategy whole_file|fixed_window|syntax\n\
         fixed_window: --window-bytes N (default 4000) --overlap-bytes N (default 400)\n\
         syntax: --max-item-bytes N (default 32768)\n\
         run adds: --query-pack PATH --routes a,b --top-k N --state-root PATH\n\
         --repo-id ID --revision-id ID --generation N --model NAME --model-revision REV\n\
         --runner-name NAME --runner-revision REV --run-id ID\n\
         --blinding isolated|attested --isolation-method TEXT --access-block-log TEXT\n\
         --out PATH [--searchd-bin PATH] [--embedder NAME] [--max-file-bytes N]\n\
         [--io-timeout-secs N] [--ready-timeout-secs N]"
    );
}

struct Args {
    positional: Vec<String>,
    flags: BTreeMap<String, String>,
}

fn parse_args(argv: &[String]) -> BenchResult<Args> {
    let mut positional = Vec::new();
    let mut flags: BTreeMap<String, String> = BTreeMap::new();
    let mut index = 1;
    while index < argv.len() {
        let arg = &argv[index];
        if arg == "--help" || arg == "-h" {
            print_help();
            std::process::exit(0);
        }
        if let Some(name) = arg.strip_prefix("--") {
            let Some(value) = argv.get(index + 1) else {
                return Err(usage_error(format!("flag --{name} lacks a value")));
            };
            if value.starts_with("--") {
                return Err(usage_error(format!("flag --{name} lacks a value")));
            }
            if flags.insert(name.to_string(), value.clone()).is_some() {
                return Err(usage_error(format!("duplicate flag --{name}")));
            }
            index += 2;
        } else {
            positional.push(arg.clone());
            index += 1;
        }
    }
    Ok(Args { positional, flags })
}

fn required(args: &Args, name: &str) -> BenchResult<String> {
    args.flags
        .get(name)
        .cloned()
        .ok_or_else(|| usage_error(format!("missing required flag --{name}")))
}

fn optional_u64(args: &Args, name: &str, default: u64) -> BenchResult<u64> {
    match args.flags.get(name) {
        None => Ok(default),
        Some(raw) => raw
            .parse::<u64>()
            .map_err(|_| usage_error(format!("flag --{name} must be an unsigned integer"))),
    }
}

fn optional_usize(args: &Args, name: &str, default: usize) -> BenchResult<usize> {
    match args.flags.get(name) {
        None => Ok(default),
        Some(raw) => raw
            .parse::<usize>()
            .map_err(|_| usage_error(format!("flag --{name} must be an unsigned integer"))),
    }
}

fn reject_unknown(args: &Args, allowed: &[&str]) -> BenchResult<()> {
    let known: BTreeSet<&str> = allowed.iter().copied().collect();
    for key in args.flags.keys() {
        if !known.contains(key.as_str()) {
            return Err(usage_error(format!("unknown flag --{key}")));
        }
    }
    Ok(())
}

fn parse_routes(raw: &str) -> BenchResult<Vec<&'static str>> {
    let mut routes = Vec::new();
    for part in raw.split(',') {
        let name = part.trim();
        let known = KNOWN_ROUTES
            .iter()
            .find(|candidate| **candidate == name)
            .copied()
            .ok_or_else(|| usage_error(format!("unknown route: {name}")))?;
        if !routes.contains(&known) {
            routes.push(known);
        }
    }
    if routes.is_empty() {
        return Err(usage_error("at least one route is required".to_string()));
    }
    Ok(routes)
}

struct ChunkSelection {
    name: String,
    config: String,
    chunks: BTreeMap<String, Vec<quanta_index_retrieval_bench::chunking::Chunk>>,
    coverage: CoverageReport,
}

fn chunk_with_strategy(
    strategy: &str,
    args: &Args,
    files: &[SourceFile],
) -> BenchResult<ChunkSelection> {
    match strategy {
        STRATEGY_WHOLE_FILE => {
            let chunker = WholeFileChunker;
            let (chunks, coverage) = chunk_corpus(&chunker, files)?;
            Ok(ChunkSelection {
                name: chunker.name().to_string(),
                config: chunker.config(),
                chunks,
                coverage,
            })
        }
        STRATEGY_FIXED_WINDOW => {
            let chunker = FixedWindowChunker::new(
                optional_usize(args, "window-bytes", 4000)?,
                optional_usize(args, "overlap-bytes", 400)?,
            );
            let (chunks, coverage) = chunk_corpus(&chunker, files)?;
            Ok(ChunkSelection {
                name: chunker.name().to_string(),
                config: chunker.config(),
                chunks,
                coverage,
            })
        }
        STRATEGY_SYNTAX => {
            let chunker = SyntaxChunker::new(optional_usize(
                args,
                "max-item-bytes",
                quanta_index_retrieval_bench::chunking::syntax::DEFAULT_MAX_ITEM_BYTES,
            )?);
            let (chunks, coverage) = chunk_corpus(&chunker, files)?;
            Ok(ChunkSelection {
                name: chunker.name().to_string(),
                config: chunker.config(),
                chunks,
                coverage,
            })
        }
        other => Err(usage_error(format!("unknown strategy: {other}"))),
    }
}

fn write_json(path: &Path, value: &serde_json::Value) -> BenchResult<()> {
    let rendered = serde_json::to_string_pretty(value).map_err(|err| BenchError::Json {
        path: path.display().to_string(),
        message: err.to_string(),
    })?;
    std::fs::write(path, format!("{rendered}\n")).map_err(|err| BenchError::Io {
        path: path.display().to_string(),
        message: err.to_string(),
    })
}

fn run_chunk(args: &Args) -> BenchResult<()> {
    reject_unknown(
        args,
        &[
            "repo",
            "manifest",
            "strategy",
            "window-bytes",
            "overlap-bytes",
            "max-item-bytes",
            "max-file-bytes",
            "out",
        ],
    )?;
    let repo = PathBuf::from(required(args, "repo")?);
    let manifest = load_manifest(&PathBuf::from(required(args, "manifest")?))?;
    let limits = CorpusLimits {
        max_file_bytes: optional_u64(
            args,
            "max-file-bytes",
            CorpusLimits::default().max_file_bytes,
        )?,
    };
    let files = load_corpus(&repo, &manifest, &limits)?;
    let selection = chunk_with_strategy(&required(args, "strategy")?, args, &files)?;
    let out = PathBuf::from(required(args, "out")?);
    let mut file_entries = serde_json::Map::new();
    for (path, chunks) in &selection.chunks {
        let items: Vec<serde_json::Value> = chunks
            .iter()
            .map(|chunk| {
                serde_json::json!({
                    "chunk_id": chunk.chunk_id,
                    "start_byte": chunk.start_byte,
                    "end_byte": chunk.end_byte,
                    "start_line": chunk.start_line,
                    "end_line": chunk.end_line,
                    "fallback": chunk.fallback,
                    "text_sha256": sha256_hex(chunk.text.as_bytes()),
                })
            })
            .collect();
        assert!(
            file_entries
                .insert(path.clone(), serde_json::Value::Array(items))
                .is_none()
        );
    }
    let coverage = &selection.coverage;
    let value = serde_json::json!({
        "strategy": selection.name,
        "strategy_config": selection.config,
        "repository_commit": manifest.repository_commit,
        "files": coverage.files,
        "chunks": coverage.chunks,
        "bytes": coverage.bytes,
        "tokens": coverage.tokens,
        "overlap_bytes": coverage.overlap_bytes,
        "uncovered_bytes": coverage.uncovered_bytes,
        "fallback_chunks": coverage.fallback_chunks,
        "per_file": file_entries,
    });
    write_json(&out, &value)?;
    println!(
        "chunked {} files into {} chunks ({} fallback, {} uncovered bytes) via {}",
        coverage.files,
        coverage.chunks,
        coverage.fallback_chunks,
        coverage.uncovered_bytes,
        selection.name
    );
    Ok(())
}

#[allow(clippy::too_many_lines)]
fn run_capture(args: &Args) -> BenchResult<()> {
    reject_unknown(
        args,
        &[
            "repo",
            "manifest",
            "strategy",
            "window-bytes",
            "overlap-bytes",
            "max-item-bytes",
            "max-file-bytes",
            "query-pack",
            "routes",
            "top-k",
            "state-root",
            "searchd-bin",
            "embedder",
            "repo-id",
            "revision-id",
            "generation",
            "model",
            "model-revision",
            "runner-name",
            "runner-revision",
            "run-id",
            "blinding",
            "isolation-method",
            "access-block-log",
            "out",
            "io-timeout-secs",
            "ready-timeout-secs",
        ],
    )?;
    let overall = Instant::now();
    let repo = PathBuf::from(required(args, "repo")?);
    let manifest = load_manifest(&PathBuf::from(required(args, "manifest")?))?;
    let pack = load_query_pack(&PathBuf::from(required(args, "query-pack")?))?;
    cross_check_manifest_pack(&manifest, &pack)?;
    let routes = parse_routes(&required(args, "routes")?)?;
    for route in &routes {
        if !pack.routes.iter().any(|name| name == route) {
            return Err(BenchError::Protocol(format!(
                "route {route} is not registered in the query pack"
            )));
        }
    }
    let top_k = u32::try_from(optional_u64(args, "top-k", 0)?)
        .map_err(|_| usage_error("flag --top-k exceeds u32 range".to_string()))?;
    if top_k == 0 {
        return Err(usage_error("flag --top-k must be positive".to_string()));
    }
    let limits = CorpusLimits {
        max_file_bytes: optional_u64(
            args,
            "max-file-bytes",
            CorpusLimits::default().max_file_bytes,
        )?,
    };
    let files = load_corpus(&repo, &manifest, &limits)?;
    let by_path: BTreeMap<String, SourceFile> = files
        .iter()
        .map(|file| (file.path.clone(), file.clone()))
        .collect();

    let chunk_start = Instant::now();
    let selection = chunk_with_strategy(&required(args, "strategy")?, args, &files)?;
    let chunk_elapsed = chunk_start.elapsed();

    let generation = optional_u64(args, "generation", 0)?;
    if generation == 0 {
        return Err(usage_error(
            "flag --generation must be positive".to_string(),
        ));
    }
    let manifest_digest = batch_manifest_digest(
        &manifest,
        &selection.name,
        &selection.config,
        &required(args, "repo-id")?,
        &required(args, "revision-id")?,
        generation,
    );
    let identity = BatchIdentity::new(
        &required(args, "repo-id")?,
        &required(args, "revision-id")?,
        generation,
        manifest_digest,
    )?;
    let (batch, assembly) = assemble_batch(&identity, &selection.chunks)?;

    let state_root = PathBuf::from(required(args, "state-root")?);
    let searchd_bin = args.flags.get("searchd-bin").map(PathBuf::from);
    let embedder = args
        .flags
        .get("embedder")
        .cloned()
        .unwrap_or_else(|| "hash-dev".to_string());
    let io_timeout = Duration::from_secs(optional_u64(
        args,
        "io-timeout-secs",
        DEFAULT_IO_TIMEOUT.as_secs(),
    )?);
    let ready_timeout = Duration::from_secs(optional_u64(
        args,
        "ready-timeout-secs",
        DEFAULT_READY_TIMEOUT.as_secs(),
    )?);
    let config = DaemonConfig {
        state_root: &state_root,
        searchd_binary: searchd_bin.as_deref(),
        embedder: &embedder,
        ready_timeout,
        io_timeout,
        history_max_generations: 8,
    };
    let boot_start = Instant::now();
    let session = DaemonSession::boot(&config)?;
    let boot_elapsed = boot_start.elapsed();
    session.assert_index_empty(&identity.repo_id, &identity.revision_id)?;

    let publish_start = Instant::now();
    let (receipt, ack) = publish_and_activate(&session, &batch, None)?;
    let publish_elapsed = publish_start.elapsed();
    if receipt.accepted_replace_scopes as usize != assembly.scopes {
        session.stop()?;
        return Err(BenchError::Protocol(format!(
            "sealed receipt accepted {} scopes but the runner published {}",
            receipt.accepted_replace_scopes, assembly.scopes
        )));
    }

    let model = required(args, "model")?;
    let model_revision = required(args, "model-revision")?;
    let mut provenance = BTreeMap::new();
    for route in routes.iter().copied() {
        assert!(
            provenance
                .insert(
                    route.to_string(),
                    RouteProvenance {
                        system: "quanta-index".to_string(),
                        model: model.clone(),
                        model_revision: model_revision.clone(),
                    },
                )
                .is_none()
        );
    }
    let query_start = Instant::now();
    let mut outcomes: BTreeMap<(String, String), QueryOutcome> = BTreeMap::new();
    for task in &pack.tasks {
        for route in routes.iter().copied() {
            let outcome = query_route(&RouteQuery {
                client: session.client(),
                route,
                query_text: &task.query,
                repo_id: &identity.repo_id,
                revision_id: &identity.revision_id,
                generation: identity.generation,
                top_k,
            });
            assert!(
                outcomes
                    .insert((task.task_id.clone(), route.to_string()), outcome)
                    .is_none()
            );
        }
    }
    let query_elapsed = query_start.elapsed();

    let identity_block = RunnerIdentity::new(
        required(args, "runner-name")?,
        required(args, "runner-revision")?,
        required(args, "run-id")?,
        required(args, "blinding")?,
        required(args, "isolation-method")?,
        required(args, "access-block-log")?,
    )?;
    let record = runner_record(
        &pack,
        &identity_block,
        &provenance,
        &outcomes,
        top_k,
        &by_path,
    )?;
    let out = PathBuf::from(required(args, "out")?);
    write_json(&out, &record)?;
    let binary = session.searchd_binary().display().to_string();
    session.stop()?;

    let mut status_counts: BTreeMap<&str, usize> = BTreeMap::new();
    for outcome in outcomes.values() {
        let status = match outcome {
            QueryOutcome::Hits { .. } => "hits",
            QueryOutcome::Failed { status, .. } => status,
        };
        *status_counts.entry(status).or_insert(0) += 1;
    }
    println!(
        "captured {} tasks x {} routes via {} (receipt gen {}, ack active {:?}); chunk={}ms boot={}ms publish={}ms query={}ms total={}ms; outcomes={status_counts:?}; binary={binary}",
        pack.tasks.len(),
        routes.len(),
        selection.name,
        receipt.generation.get(),
        ack.active,
        chunk_elapsed.as_millis(),
        boot_elapsed.as_millis(),
        publish_elapsed.as_millis(),
        query_elapsed.as_millis(),
        overall.elapsed().as_millis(),
    );
    Ok(())
}

fn batch_manifest_digest(
    manifest: &quanta_index_retrieval_bench::corpus::Manifest,
    strategy: &str,
    config: &str,
    repo_id: &str,
    revision_id: &str,
    generation: u64,
) -> String {
    let mut raw = b"retrieval-bench-batch:v1".to_vec();
    raw.push(0);
    for part in [
        manifest.repository_commit.as_str(),
        &quanta_index_retrieval_bench::corpus::universe_digest(&manifest.files),
        strategy,
        config,
        repo_id,
        revision_id,
        &generation.to_string(),
    ] {
        raw.extend_from_slice(part.as_bytes());
        raw.push(0);
    }
    sha256_hex(&raw)
}

fn cross_check_manifest_pack(
    manifest: &quanta_index_retrieval_bench::corpus::Manifest,
    pack: &QueryPack,
) -> BenchResult<()> {
    if manifest.repository_commit != pack.repository_commit {
        return Err(BenchError::Protocol(
            "manifest commit differs from query-pack commit".to_string(),
        ));
    }
    if pack.file_universe.is_empty() {
        return Ok(());
    }
    let manifest_rows: BTreeSet<(&str, &str)> = manifest
        .files
        .iter()
        .map(|file| (file.path.as_str(), file.file_sha256.as_str()))
        .collect();
    let pack_rows: BTreeSet<(&str, &str)> = pack
        .file_universe
        .iter()
        .map(|(path, digest)| (path.as_str(), digest.as_str()))
        .collect();
    // Exact set equality: any drift fails the run before expensive capture.
    if manifest_rows != pack_rows {
        let only_manifest: Vec<&(&str, &str)> =
            manifest_rows.difference(&pack_rows).take(5).collect();
        let only_pack: Vec<&(&str, &str)> = pack_rows.difference(&manifest_rows).take(5).collect();
        return Err(BenchError::Protocol(format!(
            "admitted manifest differs from pack file universe (manifest-only sample: {only_manifest:?}; pack-only sample: {only_pack:?})"
        )));
    }
    Ok(())
}

fn main() {
    let argv: Vec<String> = std::env::args().collect();
    let outcome = (|| -> BenchResult<()> {
        let args = parse_args(&argv)?;
        if args.positional.len() != 1 {
            print_help();
            return Err(usage_error(
                "expected exactly one subcommand: run|chunk".to_string(),
            ));
        }
        match args.positional[0].as_str() {
            "run" => run_capture(&args),
            "chunk" => run_chunk(&args),
            other => {
                print_help();
                Err(usage_error(format!("unknown subcommand: {other}")))
            }
        }
    })();
    if let Err(err) = outcome {
        eprintln!("ERROR: {err}");
        std::process::exit(2);
    }
}
