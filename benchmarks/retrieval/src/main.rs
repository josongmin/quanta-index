//! `quanta-index-retrieval-bench`: real-repository SDK runner binary.
//!
//! `run` loads an admitted manifest, chunks it, boots a real `searchd`,
//! publishes through the public SDK, queries SDK routes and emits a v3
//! runner record. `chunk` inspects chunking without a daemon. Unknown flags
//! fail; nothing is guessed.

#![forbid(unsafe_code)]

use std::collections::{BTreeMap, BTreeSet};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use quanta_index_retrieval_bench::batch::{
    BatchIdentity, activation_digest, assemble_batch, receipt_digest,
};
use quanta_index_retrieval_bench::chunking::{
    Chunker, CoverageReport, STRATEGY_BRACE_HEURISTIC, STRATEGY_FIXED_WINDOW_LINE_ALIGNED,
    STRATEGY_FIXED_WINDOW_STRICT, STRATEGY_WHOLE_FILE, chunk_corpus,
    fixed_window::FixedWindowChunker, fixed_window::StrictWindowChunker, record_strategy_name,
    syntax::SyntaxChunker, whole_file::WholeFileChunker,
};
use quanta_index_retrieval_bench::corpus::{
    CorpusLimits, Manifest, SourceFile, load_corpus, load_manifest, verify_checkout,
    verify_materialized_corpus,
};
use quanta_index_retrieval_bench::profile::EmbedderProfile;
use quanta_index_retrieval_bench::record::{
    CaptureProvenance, QueryPack, RouteProvenance, RunnerIdentity, RunnerRecordInput,
    load_query_pack, runner_record,
};
use quanta_index_retrieval_bench::schedule::QueryProtocol;
use quanta_index_retrieval_bench::sdk::{
    DEFAULT_IO_TIMEOUT, DEFAULT_READY_TIMEOUT, DaemonConfig, DaemonSession, QueryOutcome,
    RouteQuery, publish_and_activate, query_route, resolve_searchd_binary, verify_searchd_digest,
};
use quanta_index_retrieval_bench::{BenchError, BenchResult, sha256_hex};

const KNOWN_ROUTES: [&str; 3] = ["lexical", "semantic", "hybrid"];

fn usage_error(mut message: String) -> BenchError {
    message.push_str(" (see --help)");
    BenchError::Config(message)
}

fn print_help() -> BenchResult<()> {
    std::io::stdout()
        .write_all(
            b"quanta-index-retrieval-bench run|chunk [flags]\n\
         \n\
         run: manifest -> chunks -> real searchd publish/activate -> SDK queries -> v3 record\n\
         chunk: manifest -> chunks + coverage JSON (no daemon)\n\
         \n\
         shared: --repo PATH --manifest PATH --strategy whole_file|fixed_window_strict|fixed_window_line_aligned|brace_heuristic\n\
         fixed_window_*: --window-bytes N (default 4000) --overlap-bytes N (default 400)\n\
         brace_heuristic: --max-item-bytes N (default 32768)\n\
         run adds: --query-pack PATH --routes a,b --top-k N --state-root PATH\n\
         [--query-protocol PATH]\n\
         --repo-id ID --revision-id ID --generation N\n\
         --runner-name NAME --runner-revision REV --run-id ID\n\
         --blinding attested|isolated --isolation-method TEXT --access-block-log TEXT\n\
         [--materialized-corpus-sha256 HEX]\n\
         --searchd-bin PATH --searchd-expected-sha256 HEX\n\
         --out PATH [--metrics-out PATH] [--embedder potion-code|hash-dev]\n\
         [--max-file-bytes N]\n\
         [--io-timeout-secs N] [--ready-timeout-secs N]\n",
        )
        .map_err(|err| BenchError::Io {
            path: "stdout".to_string(),
            message: err.to_string(),
        })
}

fn stdout_line(message: &str) -> BenchResult<()> {
    writeln!(std::io::stdout().lock(), "{message}").map_err(|err| BenchError::Io {
        path: "stdout".to_string(),
        message: err.to_string(),
    })
}

struct Args {
    positional: Vec<String>,
    flags: BTreeMap<String, String>,
    help: bool,
}

fn parse_args(argv: &[String]) -> BenchResult<Args> {
    let mut positional = Vec::new();
    let mut flags: BTreeMap<String, String> = BTreeMap::new();
    let mut index = 1_usize;
    while index < argv.len() {
        let arg = argv
            .get(index)
            .ok_or_else(|| usage_error("argument index is absent".to_string()))?;
        if arg == "--help" || arg == "-h" {
            return Ok(Args {
                positional,
                flags,
                help: true,
            });
        }
        if let Some(name) = arg.strip_prefix("--") {
            let Some(value) = argv.get(index.saturating_add(1)) else {
                return Err(usage_error(format!("flag --{name} lacks a value")));
            };
            if value.starts_with("--") {
                return Err(usage_error(format!("flag --{name} lacks a value")));
            }
            if flags.insert(name.to_string(), value.clone()).is_some() {
                return Err(usage_error(format!("duplicate flag --{name}")));
            }
            index = index.saturating_add(2);
        } else {
            positional.push(arg.clone());
            index = index.saturating_add(1);
        }
    }
    Ok(Args {
        positional,
        flags,
        help: false,
    })
}

fn required(args: &Args, name: &str) -> BenchResult<String> {
    args.flags
        .get(name)
        .cloned()
        .ok_or_else(|| usage_error(format!("missing required flag --{name}")))
}

fn optional_u64(args: &Args, name: &str, default: u64) -> BenchResult<u64> {
    args.flags.get(name).map_or(Ok(default), |raw| {
        raw.parse::<u64>()
            .map_err(|err| usage_error(format!("flag --{name} must be an unsigned integer: {err}")))
    })
}

fn optional_usize(args: &Args, name: &str, default: usize) -> BenchResult<usize> {
    args.flags.get(name).map_or(Ok(default), |raw| {
        raw.parse::<usize>()
            .map_err(|err| usage_error(format!("flag --{name} must be an unsigned integer: {err}")))
    })
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

fn verify_capture_corpus(args: &Args, repo: &Path, manifest: &Manifest) -> BenchResult<()> {
    if let Some(proof) = args.flags.get("materialized-corpus-sha256") {
        let valid = proof.len() == 64
            && proof
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte));
        if !valid || required(args, "blinding")? != "isolated" {
            return Err(BenchError::Config(
                "materialized corpus requires isolated blinding and a lowercase sha256 proof"
                    .to_string(),
            ));
        }
        verify_materialized_corpus(repo, manifest)
    } else {
        verify_checkout(repo, manifest)
    }
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
    config_value: serde_json::Value,
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
                config_value: chunker.config_value(),
                chunks,
                coverage,
            })
        }
        STRATEGY_FIXED_WINDOW_STRICT => {
            let chunker = StrictWindowChunker::new(
                optional_usize(args, "window-bytes", 4000)?,
                optional_usize(args, "overlap-bytes", 400)?,
            );
            let (chunks, coverage) = chunk_corpus(&chunker, files)?;
            Ok(ChunkSelection {
                name: chunker.name().to_string(),
                config: chunker.config(),
                config_value: chunker.config_value(),
                chunks,
                coverage,
            })
        }
        STRATEGY_FIXED_WINDOW_LINE_ALIGNED => {
            let chunker = FixedWindowChunker::new(
                optional_usize(args, "window-bytes", 4000)?,
                optional_usize(args, "overlap-bytes", 400)?,
            );
            let (chunks, coverage) = chunk_corpus(&chunker, files)?;
            Ok(ChunkSelection {
                name: chunker.name().to_string(),
                config: chunker.config(),
                config_value: chunker.config_value(),
                chunks,
                coverage,
            })
        }
        STRATEGY_BRACE_HEURISTIC => {
            let chunker = SyntaxChunker::new(optional_usize(
                args,
                "max-item-bytes",
                quanta_index_retrieval_bench::chunking::syntax::DEFAULT_MAX_ITEM_BYTES,
            )?);
            let (chunks, coverage) = chunk_corpus(&chunker, files)?;
            Ok(ChunkSelection {
                name: chunker.name().to_string(),
                config: chunker.config(),
                config_value: chunker.config_value(),
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
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .map_err(|err| BenchError::Io {
            path: path.display().to_string(),
            message: format!("refusing to overwrite benchmark evidence: {err}"),
        })?;
    file.write_all(format!("{rendered}\n").as_bytes())
        .map_err(|err| BenchError::Io {
            path: path.display().to_string(),
            message: err.to_string(),
        })
}

fn require_external_path(repo: &Path, path: &Path, label: &str) -> BenchResult<()> {
    if !path.is_absolute() {
        return Err(BenchError::Config(format!(
            "{label} must be an absolute path"
        )));
    }
    if std::fs::symlink_metadata(path).is_ok_and(|meta| meta.file_type().is_symlink()) {
        return Err(BenchError::Config(format!("{label} must not be a symlink")));
    }
    let root = std::fs::canonicalize(repo).map_err(|err| BenchError::Io {
        path: repo.display().to_string(),
        message: err.to_string(),
    })?;
    let parent = path
        .parent()
        .ok_or_else(|| BenchError::Config(format!("{label} has no parent")))?;
    let canonical_parent = std::fs::canonicalize(parent).map_err(|err| BenchError::Io {
        path: parent.display().to_string(),
        message: err.to_string(),
    })?;
    if canonical_parent.starts_with(&root) {
        return Err(BenchError::Config(format!(
            "{label} must be outside the frozen repository"
        )));
    }
    Ok(())
}

fn validate_blinding_claim(
    blinding: &str,
    isolation_method: &str,
    access_block_log: &str,
) -> BenchResult<()> {
    match blinding {
        "attested" => Ok(()),
        "isolated"
            if isolation_method == "macos-seatbelt-v1"
                && access_block_log
                    .strip_prefix("sha256:")
                    .is_some_and(|value| {
                        value.len() == 64
                            && value
                                .bytes()
                                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
                    }) => Ok(()),
        "isolated" => Err(BenchError::Config(
            "isolated records require the macos-seatbelt-v1 backend and a sha256-bound access proof; the external verdict remains the isolation authority"
                .to_string(),
        )),
        _ => Err(BenchError::Config(
            "--blinding must be attested or isolated".to_string(),
        )),
    }
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
    verify_checkout(&repo, &manifest)?;
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
    require_external_path(&repo, &out, "--out")?;
    if out.exists() {
        return Err(BenchError::Config(format!(
            "--out already exists: {}",
            out.display()
        )));
    }
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
        if file_entries
            .insert(path.clone(), serde_json::Value::Array(items))
            .is_some()
        {
            return Err(BenchError::Protocol(format!(
                "duplicate chunk path: {path}"
            )));
        }
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
    verify_checkout(&repo, &manifest)?;
    write_json(&out, &value)?;
    stdout_line(&format!(
        "chunked {} files into {} chunks ({} fallback, {} uncovered bytes) via {}",
        coverage.files,
        coverage.chunks,
        coverage.fallback_chunks,
        coverage.uncovered_bytes,
        selection.name
    ))?;
    Ok(())
}

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
            "query-protocol",
            "routes",
            "top-k",
            "state-root",
            "searchd-bin",
            "searchd-expected-sha256",
            "embedder",
            "repo-id",
            "revision-id",
            "generation",
            "runner-name",
            "runner-revision",
            "run-id",
            "blinding",
            "isolation-method",
            "access-block-log",
            "metrics-out",
            "out",
            "io-timeout-secs",
            "ready-timeout-secs",
            "materialized-corpus-sha256",
            "model-dir",
        ],
    )?;
    let overall = Instant::now();
    let repo = PathBuf::from(required(args, "repo")?);
    let manifest = load_manifest(&PathBuf::from(required(args, "manifest")?))?;
    verify_capture_corpus(args, &repo, &manifest)?;
    let pack = load_query_pack(&PathBuf::from(required(args, "query-pack")?))?;
    cross_check_manifest_pack(&manifest, &pack)?;
    let expected_task_ids: Vec<String> =
        pack.tasks.iter().map(|task| task.task_id.clone()).collect();
    let query_protocol = args
        .flags
        .get("query-protocol")
        .map(|path| QueryProtocol::load(Path::new(path), &expected_task_ids))
        .transpose()?;
    let routes = parse_routes(&required(args, "routes")?)?;
    let selected: BTreeSet<&str> = routes.iter().copied().collect();
    let registered: BTreeSet<&str> = pack.routes.iter().map(String::as_str).collect();
    if selected != registered {
        return Err(BenchError::Protocol(format!(
            "runner routes {selected:?} differ from query-pack routes {registered:?}"
        )));
    }
    let top_k = u32::try_from(optional_u64(args, "top-k", 0)?)
        .map_err(|err| usage_error(format!("flag --top-k exceeds u32 range: {err}")))?;
    if top_k == 0 {
        return Err(usage_error("flag --top-k must be positive".to_string()));
    }
    if top_k != pack.contract_top_k {
        return Err(BenchError::Protocol(format!(
            "flag --top-k={top_k} differs from the query-pack comparison contract top_k={}",
            pack.contract_top_k
        )));
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
    let discovery_elapsed = overall.elapsed();

    let chunk_start = Instant::now();
    let selection = chunk_with_strategy(&required(args, "strategy")?, args, &files)?;
    let chunk_elapsed = chunk_start.elapsed();
    let mut chunks_by_id = BTreeMap::new();
    for chunk in selection.chunks.values().flatten() {
        if chunks_by_id
            .insert(chunk.chunk_id.clone(), chunk.clone())
            .is_some()
        {
            return Err(BenchError::Protocol(format!(
                "duplicate published chunk ID: {}",
                chunk.chunk_id
            )));
        }
    }

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
    require_external_path(&repo, &state_root, "--state-root")?;
    let out = PathBuf::from(required(args, "out")?);
    require_external_path(&repo, &out, "--out")?;
    if out.exists() {
        return Err(BenchError::Config(format!(
            "--out already exists: {}",
            out.display()
        )));
    }
    let metrics_out = args.flags.get("metrics-out").map(PathBuf::from);
    if let Some(path) = &metrics_out {
        require_external_path(&repo, path, "--metrics-out")?;
        if path.exists() {
            return Err(BenchError::Config(format!(
                "--metrics-out already exists: {}",
                path.display()
            )));
        }
    }
    let searchd_bin =
        resolve_searchd_binary(args.flags.get("searchd-bin").map(PathBuf::from).as_deref())?;
    let searchd_digest =
        verify_searchd_digest(&searchd_bin, &required(args, "searchd-expected-sha256")?)?;
    let profile = EmbedderProfile::resolve(args.flags.get("embedder").map(String::as_str))?;
    let model_dir = args.flags.get("model-dir").map(PathBuf::from);
    if model_dir.as_ref().is_some_and(|path| !path.is_dir()) {
        return Err(BenchError::Config(
            "--model-dir must name an existing directory".to_string(),
        ));
    }
    let blinding = required(args, "blinding")?;
    let isolation_method = required(args, "isolation-method")?;
    let access_block_log = required(args, "access-block-log")?;
    validate_blinding_claim(&blinding, &isolation_method, &access_block_log)?;
    let identity_block = RunnerIdentity::new(
        required(args, "runner-name")?,
        required(args, "runner-revision")?,
        required(args, "run-id")?,
        blinding,
        isolation_method,
        access_block_log,
    )?;
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
        searchd_binary: Some(searchd_bin.as_path()),
        embedder: profile.selector,
        model_dir: model_dir.as_deref(),
        repo_id: &identity.repo_id,
        revision_id: &identity.revision_id,
        ready_timeout,
        io_timeout,
        history_max_generations: 8,
    };
    let boot_start = Instant::now();
    // Boot proves the fresh index empty; readiness precedes publish.
    let session = DaemonSession::boot(&config)?;
    let boot_elapsed = boot_start.elapsed();

    let publish_start = Instant::now();
    let (receipt, ack) = publish_and_activate(&session, &batch, &identity, None)?;
    let publish_elapsed = publish_start.elapsed();
    let accepted_scopes = usize::try_from(receipt.accepted_replace_scopes).map_err(|err| {
        BenchError::Protocol(format!("receipt scope count cannot fit usize: {err}"))
    })?;
    if accepted_scopes != assembly.scopes {
        session.stop()?;
        return Err(BenchError::Protocol(format!(
            "sealed receipt accepted {} scopes but the runner published {}",
            receipt.accepted_replace_scopes, assembly.scopes
        )));
    }

    let record_strategy = record_strategy_name(&selection.name).ok_or_else(|| {
        BenchError::Protocol(format!(
            "chunk strategy {} has no frozen v3 record name",
            selection.name
        ))
    })?;
    let receipt_binding = receipt_digest(&receipt)?;
    let activation_binding = activation_digest(&ack)?;
    let runner_digest = runner_binary_digest()?;
    let run_id = required(args, "run-id")?;
    let runner_name = required(args, "runner-name")?;
    let mut provenance: BTreeMap<String, RouteProvenance> = BTreeMap::new();
    let mut captures: BTreeMap<String, CaptureProvenance> = BTreeMap::new();
    for route in routes.iter().copied() {
        let (model, model_revision) = if route == "lexical" {
            ("none:lexical", "not-applicable")
        } else {
            (profile.model_id, profile.model_revision)
        };
        let capture_id = format!("{run_id}-{route}");
        if provenance
            .insert(
                route.to_string(),
                RouteProvenance {
                    capture_id: capture_id.clone(),
                },
            )
            .is_some()
        {
            return Err(BenchError::Protocol(format!("duplicate route: {route}")));
        }
        if captures
            .insert(
                capture_id.clone(),
                CaptureProvenance {
                    chunk_strategy: record_strategy.to_string(),
                    chunk_config: selection.config_value.clone(),
                    runner_binary_name: runner_name.clone(),
                    runner_binary_digest: runner_digest.clone(),
                    searchd_binary_digest: searchd_digest.clone(),
                    generation: identity.generation.get(),
                    receipt_digest: receipt_binding.clone(),
                    activation_digest: activation_binding.clone(),
                    model: model.to_string(),
                    model_revision: model_revision.to_string(),
                },
            )
            .is_some()
        {
            return Err(BenchError::Protocol(format!(
                "duplicate capture_id: {capture_id}"
            )));
        }
    }
    let task_queries: BTreeMap<&str, &str> = pack
        .tasks
        .iter()
        .map(|task| (task.task_id.as_str(), task.query.as_str()))
        .collect();
    let mut outcomes: BTreeMap<(String, String), QueryOutcome> = BTreeMap::new();
    let mut warm_latencies_ms: BTreeMap<String, BTreeMap<String, Vec<f64>>> = BTreeMap::new();
    let mut cold_latencies_ms: BTreeMap<String, f64> = BTreeMap::new();
    let mut warmup_elapsed = Duration::ZERO;
    let first_query_elapsed;
    let warm_query_elapsed;
    if let Some(protocol) = &query_protocol {
        let cold_start = Instant::now();
        let cold_query = task_queries
            .get(protocol.cold_probe_task_id.as_str())
            .ok_or_else(|| BenchError::Protocol("cold probe task disappeared".to_string()))?;
        for route in routes.iter().copied() {
            let outcome = query_route(&RouteQuery {
                client: session.client(),
                route,
                query_text: cold_query,
                repo_id: &identity.repo_id,
                revision_id: &identity.revision_id,
                generation: identity.generation,
                top_k,
            });
            let latency = match &outcome {
                QueryOutcome::Hits { latency, .. } => *latency,
                QueryOutcome::Failed { status, code, .. } => {
                    return Err(BenchError::Protocol(format!(
                        "cold probe failed for route {route}: {status}/{code}"
                    )));
                }
            };
            let previous =
                cold_latencies_ms.insert(route.to_string(), latency.as_secs_f64() * 1000.0);
            if previous.is_some() {
                return Err(BenchError::Protocol(format!(
                    "duplicate cold latency for route {route}"
                )));
            }
        }
        first_query_elapsed = cold_start.elapsed();

        let warmup_start = Instant::now();
        for schedule in &protocol.warmup_schedules {
            for task_id in schedule {
                let query = task_queries
                    .get(task_id.as_str())
                    .ok_or_else(|| BenchError::Protocol("warmup task disappeared".to_string()))?;
                for route in routes.iter().copied() {
                    if let QueryOutcome::Failed { status, code, .. } = query_route(&RouteQuery {
                        client: session.client(),
                        route,
                        query_text: query,
                        repo_id: &identity.repo_id,
                        revision_id: &identity.revision_id,
                        generation: identity.generation,
                        top_k,
                    }) {
                        return Err(BenchError::Protocol(format!(
                            "warmup query failed for {task_id}/{route}: {status}/{code}"
                        )));
                    }
                }
            }
        }
        warmup_elapsed = warmup_start.elapsed();

        let measurement_start = Instant::now();
        for (repetition, schedule) in protocol.measurement_schedules.iter().enumerate() {
            for task_id in schedule {
                let query = task_queries.get(task_id.as_str()).ok_or_else(|| {
                    BenchError::Protocol("measurement task disappeared".to_string())
                })?;
                for route in routes.iter().copied() {
                    let outcome = query_route(&RouteQuery {
                        client: session.client(),
                        route,
                        query_text: query,
                        repo_id: &identity.repo_id,
                        revision_id: &identity.revision_id,
                        generation: identity.generation,
                        top_k,
                    });
                    let latency = match &outcome {
                        QueryOutcome::Hits { latency, .. } => *latency,
                        QueryOutcome::Failed { status, code, .. } => {
                            return Err(BenchError::Protocol(format!(
                                "measurement query failed for {task_id}/{route}: {status}/{code}"
                            )));
                        }
                    };
                    warm_latencies_ms
                        .entry(route.to_string())
                        .or_default()
                        .entry(task_id.clone())
                        .or_default()
                        .push(latency.as_secs_f64() * 1000.0);
                    if repetition == 0
                        && outcomes
                            .insert((task_id.clone(), route.to_string()), outcome)
                            .is_some()
                    {
                        return Err(BenchError::Protocol(format!(
                            "duplicate outcome for task {task_id} route {route}"
                        )));
                    }
                }
            }
        }
        warm_query_elapsed = measurement_start.elapsed();
    } else {
        let query_start = Instant::now();
        let mut first = Duration::ZERO;
        for task in &pack.tasks {
            for route in routes.iter().copied() {
                let single_query_start = Instant::now();
                let outcome = query_route(&RouteQuery {
                    client: session.client(),
                    route,
                    query_text: &task.query,
                    repo_id: &identity.repo_id,
                    revision_id: &identity.revision_id,
                    generation: identity.generation,
                    top_k,
                });
                if first.is_zero() {
                    first = single_query_start.elapsed();
                }
                if outcomes
                    .insert((task.task_id.clone(), route.to_string()), outcome)
                    .is_some()
                {
                    return Err(BenchError::Protocol(format!(
                        "duplicate outcome for task {} route {route}",
                        task.task_id
                    )));
                }
            }
        }
        first_query_elapsed = first;
        warm_query_elapsed = query_start.elapsed().saturating_sub(first);
    }

    let record = runner_record(&RunnerRecordInput {
        pack: &pack,
        identity: &identity_block,
        provenance: &provenance,
        captures: &captures,
        outcomes: &outcomes,
        top_k,
        files: &by_path,
        chunks_by_id: &chunks_by_id,
    })?;
    verify_capture_corpus(args, &repo, &manifest)?;
    let binary = session.searchd_binary().display().to_string();
    session.stop()?;
    let overall_elapsed = overall.elapsed();
    let phase_sum = discovery_elapsed
        .checked_add(chunk_elapsed)
        .and_then(|value| value.checked_add(boot_elapsed))
        .and_then(|value| value.checked_add(publish_elapsed))
        .and_then(|value| value.checked_add(first_query_elapsed))
        .and_then(|value| value.checked_add(warmup_elapsed))
        .and_then(|value| value.checked_add(warm_query_elapsed))
        .ok_or_else(|| BenchError::Protocol("runner phase duration overflow".to_string()))?;
    let phase_sum_ms = phase_sum.as_secs_f64() * 1000.0;
    let total_ms = overall_elapsed.as_secs_f64() * 1000.0;
    let rendered_record =
        serde_json::to_string_pretty(&record).map_err(|err| BenchError::Json {
            path: out.display().to_string(),
            message: err.to_string(),
        })?;
    let record_digest = sha256_hex(format!("{rendered_record}\n").as_bytes());
    let mut phase_metrics = serde_json::json!({
        "schema_version": 1,
        "system": "quanta",
        "timing_layer": "runner_monotonic_wall_v1",
        "strategy": selection.name,
        "record_sha256": record_digest,
        "runner_binary_sha256": runner_digest,
        "task_count": pack.tasks.len(),
        "route_count": routes.len(),
        "file_count": selection.coverage.files,
        "chunk_count": selection.coverage.chunks,
        "query_schedule": pack.tasks.iter().map(|task| task.task_id.as_str()).collect::<Vec<_>>(),
        "warmup_passes": query_protocol.as_ref().map_or(0, |value| value.warmup_schedules.len()),
        "measurement_repetitions": query_protocol.as_ref().map_or(1, |value| value.measurement_schedules.len()),
        "phases_ms": {
            "discovery": discovery_elapsed.as_secs_f64() * 1000.0,
            "chunk": chunk_elapsed.as_secs_f64() * 1000.0,
            "model_provider_prepare": boot_elapsed.as_secs_f64() * 1000.0,
            "embed_publish_seal_activate": publish_elapsed.as_secs_f64() * 1000.0,
            "first_query": first_query_elapsed.as_secs_f64() * 1000.0,
            "warmup": warmup_elapsed.as_secs_f64() * 1000.0,
            "warm_query": warm_query_elapsed.as_secs_f64() * 1000.0,
            "unattributed": (total_ms - phase_sum_ms).max(0.0),
        },
        "total_ms": total_ms,
    });
    if let Some(protocol) = &query_protocol {
        let object = phase_metrics.as_object_mut().ok_or_else(|| {
            BenchError::Protocol("phase metrics must serialize as an object".to_string())
        })?;
        let phases = object
            .get_mut("phases_ms")
            .and_then(serde_json::Value::as_object_mut)
            .ok_or_else(|| BenchError::Protocol("phase map disappeared".to_string()))?;
        let cold = phases
            .remove("first_query")
            .ok_or_else(|| BenchError::Protocol("cold query phase disappeared".to_string()))?;
        if phases.insert("cold_query".to_string(), cold).is_some() {
            return Err(BenchError::Protocol(
                "duplicate cold query phase".to_string(),
            ));
        }
        if object
            .insert(
                "query_protocol".to_string(),
                serde_json::to_value(protocol)
                    .map_err(|err| BenchError::Protocol(err.to_string()))?,
            )
            .is_some()
        {
            return Err(BenchError::Protocol(
                "duplicate query protocol evidence".to_string(),
            ));
        }
        if object
            .insert(
                "warm_latencies_ms".to_string(),
                serde_json::to_value(&warm_latencies_ms)
                    .map_err(|err| BenchError::Protocol(err.to_string()))?,
            )
            .is_some()
        {
            return Err(BenchError::Protocol(
                "duplicate warm latency evidence".to_string(),
            ));
        }
        if object
            .insert(
                "cold_latencies_ms".to_string(),
                serde_json::to_value(&cold_latencies_ms)
                    .map_err(|err| BenchError::Protocol(err.to_string()))?,
            )
            .is_some()
        {
            return Err(BenchError::Protocol(
                "duplicate cold latency evidence".to_string(),
            ));
        }
    } else {
        let phases = phase_metrics
            .as_object_mut()
            .and_then(|object| object.get_mut("phases_ms"))
            .and_then(serde_json::Value::as_object_mut)
            .ok_or_else(|| BenchError::Protocol("phase map disappeared".to_string()))?;
        if phases.remove("warmup").is_none() {
            return Err(BenchError::Protocol("warmup phase disappeared".to_string()));
        }
    }
    if let Some(path) = &metrics_out {
        write_json(path, &phase_metrics)?;
    }
    // A failed owned-daemon shutdown or phase-artifact write must not leave a
    // scoreable success record. The record is the final create-new artifact.
    write_json(&out, &record)?;

    let mut status_counts: BTreeMap<&str, usize> = BTreeMap::new();
    for outcome in outcomes.values() {
        let status = match outcome {
            QueryOutcome::Hits { .. } => "hits",
            QueryOutcome::Failed { status, .. } => status,
        };
        let count = status_counts.entry(status).or_insert(0);
        *count = count
            .checked_add(1)
            .ok_or_else(|| BenchError::Protocol("outcome status count overflow".to_string()))?;
    }
    stdout_line(&format!(
        "captured {} tasks x {} routes via {} (receipt gen {}, ack active {:?}); chunk={}ms boot={}ms publish={}ms query={}ms total={}ms; outcomes={status_counts:?}; binary={binary}",
        pack.tasks.len(),
        routes.len(),
        selection.name,
        receipt.generation.get(),
        ack.active,
        chunk_elapsed.as_millis(),
        boot_elapsed.as_millis(),
        publish_elapsed.as_millis(),
        first_query_elapsed
            .saturating_add(warmup_elapsed)
            .saturating_add(warm_query_elapsed)
            .as_millis(),
        overall_elapsed.as_millis(),
    ))?;
    Ok(())
}

/// SHA-256 of the actual runner executable at capture time: the
/// `runner_binary.digest` binding. A replaced binary mid-run cannot keep
/// the old digest.
fn runner_binary_digest() -> BenchResult<String> {
    let exe = std::env::current_exe().map_err(|err| BenchError::Io {
        path: "<runner-executable>".to_string(),
        message: format!("cannot resolve the runner executable: {err}"),
    })?;
    let bytes = std::fs::read(&exe).map_err(|err| BenchError::Io {
        path: exe.display().to_string(),
        message: format!("cannot hash the runner executable: {err}"),
    })?;
    Ok(sha256_hex(&bytes))
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
        return Err(BenchError::Protocol(
            "query pack lacks an admitted file universe; cannot prove paired coverage".to_string(),
        ));
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

fn run_cli(argv: &[String]) -> BenchResult<()> {
    let parsed = parse_args(argv)?;
    if parsed.help {
        return print_help();
    }
    if parsed.positional.len() != 1 {
        print_help()?;
        return Err(usage_error(
            "expected exactly one subcommand: run|chunk".to_string(),
        ));
    }
    match parsed.positional.first().map(String::as_str) {
        Some("run") => run_capture(&parsed),
        Some("chunk") => run_chunk(&parsed),
        Some(other) => {
            print_help()?;
            Err(usage_error(format!("unknown subcommand: {other}")))
        }
        None => Err(usage_error("missing subcommand".to_string())),
    }
}

fn main() -> std::process::ExitCode {
    let argv: Vec<String> = std::env::args().collect();
    match run_cli(&argv) {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(err) => {
            let _diagnostic = writeln!(std::io::stderr().lock(), "ERROR: {err}");
            std::process::ExitCode::from(2)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn evidence_output_is_external_and_never_overwritten() {
        let parent = tempfile::tempdir().expect("tempdir");
        let repo = parent.path().join("repo");
        let external = parent.path().join("evidence");
        std::fs::create_dir(&repo).expect("repo dir");
        std::fs::create_dir(&external).expect("evidence dir");
        assert!(require_external_path(&repo, &repo.join("run.json"), "--out").is_err());
        let output = external.join("run.json");
        assert!(require_external_path(&repo, &output, "--out").is_ok());
        write_json(&output, &serde_json::json!({"run": 1})).expect("first write");
        assert!(write_json(&output, &serde_json::json!({"run": 2})).is_err());
        let saved = std::fs::read_to_string(&output).expect("saved output");
        assert!(saved.contains("\"run\": 1"));
    }

    #[test]
    fn isolated_record_requires_external_proof_binding() {
        assert!(validate_blinding_claim("attested", "attested-only", "none").is_ok());
        assert!(
            validate_blinding_claim(
                "isolated",
                "macos-seatbelt-v1",
                &format!("sha256:{}", "a".repeat(64)),
            )
            .is_ok()
        );
        assert!(validate_blinding_claim("isolated", "label-only", &"a".repeat(64)).is_err());
        assert!(
            validate_blinding_claim("isolated", "macos-seatbelt-v1", "sha256:not-hex").is_err()
        );
    }
}
