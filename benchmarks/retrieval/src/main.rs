//! `quanta-index-retrieval-bench`: real-repository SDK runner binary.
//!
//! `run` loads an admitted manifest, chunks it, boots a real `searchd`,
//! publishes through the public SDK, queries SDK routes and emits a v3
//! runner record. `chunk` inspects chunking without a daemon. Unknown flags
//! fail; nothing is guessed.

#![forbid(unsafe_code)]

mod rank_study;

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
use quanta_index_retrieval_bench::diagnostics::diagnostic_value;
use quanta_index_retrieval_bench::profile::EmbedderProfile;
use quanta_index_retrieval_bench::published_units::PublishedUnitRegistry;
use quanta_index_retrieval_bench::query_plan::{
    NlPlanConfig, QueryInputPolicy, QueryPlan, QueryPlanError, execution_profile_sha256,
    execution_profile_value, plan_query,
};
use quanta_index_retrieval_bench::record::{
    CaptureProvenance, QueryPack, RouteProvenance, RunnerIdentity, RunnerRecordInput,
    load_query_pack, result_value, runner_record,
};
use quanta_index_retrieval_bench::schedule::QueryProtocol;
use quanta_index_retrieval_bench::sdk::{
    DEFAULT_IO_TIMEOUT, DEFAULT_READY_TIMEOUT, DaemonConfig, DaemonSession, QueryOutcome,
    RouteQuery, publish_and_activate, query_route_with_policy, resolve_searchd_binary,
    verify_searchd_digest,
};
use quanta_index_retrieval_bench::symbols::{
    SymbolCoveragePolicy, SymbolPreflightOptions, preflight_corpus_symbols,
};
use quanta_index_retrieval_bench::{BenchError, BenchResult, sha256_hex};
use quanta_index_search_plane::{HybridFetchFloorPolicy, QueryStageObservationPolicy};

const KNOWN_ROUTES: [&str; 4] = ["lexical", "semantic", "hybrid", "symbol"];

fn usage_error(mut message: String) -> BenchError {
    message.push_str(" (see --help)");
    BenchError::Config(message)
}

fn print_help() -> BenchResult<()> {
    std::io::stdout()
        .write_all(
            b"quanta-index-retrieval-bench run|chunk|preflight [flags]\n\
         \n\
         run: manifest -> chunks -> real searchd publish/activate -> SDK queries -> v5 record\n\
         chunk: manifest -> chunks + coverage JSON (no daemon)\n\
         preflight: --repo PATH --manifest PATH --out PATH (all-file symbol census; no daemon)\n\
         [--symbol-coverage require-complete|allow-incomplete] (default require-complete)\n\
         [--symbol-timeout-ms N] [--max-symbols-per-file N]\n\
         [--symbol-total-timeout-ms N] [--max-symbols-total N]\n\
         [--max-symbol-diagnostics-per-file N] [--max-symbol-diagnostics N]\n\
         \n\
         shared: --repo PATH --manifest PATH --strategy whole_file|fixed_window_strict|fixed_window_line_aligned|brace_heuristic\n\
         fixed_window_*: --window-bytes N (default 4000) --overlap-bytes N (default 400)\n\
         brace_heuristic: --max-item-bytes N (default 32768)\n\
         run adds: --query-pack PATH --routes a,b --top-k N --state-root PATH\n\
         [--query-protocol PATH] [--query-input-policy native|literal|literal_file|keyword_file|substring_file|code_search_file|code_search_exact_content_file|code_search_components_file|code_search_typo_file|natural_language|natural_language_file|exact_symbol_name]\n\
         [--nl-max-tokens N] (natural_language* only; 1..128, default 32; exploratory)\n\
         [--query-stage-observation enabled|disabled] (default enabled; server query stages only)\n\
         [--experimental-hybrid-fetch-floor 25|50|100] (default 100; explicit experimental startup policy)\n\
         --repo-id ID --revision-id ID --generation N\n\
         --runner-name NAME --runner-revision REV --run-id ID\n\
         --blinding attested|isolated --isolation-method TEXT --access-block-log TEXT\n\
         [--materialized-corpus-sha256 HEX]\n\
         --searchd-bin PATH --searchd-expected-sha256 HEX\n\
         [--source-stream-id ID] [--source-event-id ID] [--source-base-event-id ID]\n\
         [--symbol-preflight-out PATH]\n\
         --out PATH --refusal-out PATH [--metrics-out PATH] [--diagnostics-out PATH] [--embedder potion-code|potion-code-full-v2|hash-dev]\n\
         [--rank-study-out PATH --rank-study-max-files N --rank-study-max-pages N --rank-study-timeout-ms N] (optional post-measurement ordinary CodeSearch study)\n\
         potion-code: historical effective 512-token V1; potion-code-full-v2: no 512-token truncation, 16 KiB/text and 4 MiB/model batch admission, rebuild required\n\
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
    write_json_bound(path, value).map(drop)
}

fn write_json_bound(path: &Path, value: &serde_json::Value) -> BenchResult<String> {
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
    let bytes = format!("{rendered}\n").into_bytes();
    file.write_all(&bytes).map_err(|err| BenchError::Io {
        path: path.display().to_string(),
        message: err.to_string(),
    })?;
    Ok(sha256_hex(&bytes))
}

fn query_plan_error_details(error: &QueryPlanError) -> serde_json::Value {
    match error {
        QueryPlanError::UnsupportedPolicy(policy) => serde_json::json!({"policy": policy}),
        QueryPlanError::InvalidSymbolName
        | QueryPlanError::EmptyTokenPlan
        | QueryPlanError::InvalidKeyword
        | QueryPlanError::InvalidCodeSearch
        | QueryPlanError::InvalidCodeSearchExactContent
        | QueryPlanError::InvalidCodeSearchTypo
        | QueryPlanError::InvalidCodeSearchComponents => serde_json::json!({}),
        QueryPlanError::InvalidSubstring { reason } => serde_json::json!({"reason": reason}),
        QueryPlanError::TokenLimitExceeded { tokens, max_tokens } => {
            serde_json::json!({"tokens": tokens, "max_tokens": max_tokens})
        }
        QueryPlanError::TokenCharacterLimitExceeded {
            chars,
            max_token_chars,
        } => serde_json::json!({"chars": chars, "max_token_chars": max_token_chars}),
        QueryPlanError::IndexTokenTooLong { bytes, max_bytes } => {
            serde_json::json!({"bytes": bytes, "max_bytes": max_bytes})
        }
        QueryPlanError::InvalidLexicalRequest {
            parser_code,
            detail,
        } => serde_json::json!({"parser_code": parser_code, "detail": detail}),
        QueryPlanError::NativeProjectionRequiresPolicy { projection } => {
            serde_json::json!({"projection": projection})
        }
    }
}

fn validated_protocol_latency(outcome: &QueryOutcome, phase: &str) -> BenchResult<Duration> {
    // A typed refusal is a measured execution outcome. The record and verdict
    // decide quality and qualification; the protocol must still reach later tasks.
    outcome
        .classification()
        .map(|_classification| outcome.latency())
        .map_err(|message| BenchError::Protocol(format!("invalid {phase} outcome: {message}")))
}

fn write_query_plan_refusal(
    path: &Path,
    policy: &str,
    execution_profile_sha256: Option<&str>,
    task_id: Option<&str>,
    original_query_sha256: Option<&str>,
    error: &QueryPlanError,
) -> BenchResult<()> {
    write_json(
        path,
        &serde_json::json!({
            "schema_version": 1,
            "kind": "quanta_retrieval_query_plan_refusal",
            "phase": "query_plan",
            "task_id": task_id,
            "original_query_sha256": original_query_sha256,
            "policy": policy,
            "execution_profile_sha256": execution_profile_sha256,
            "error": {
                "code": error.code(),
                "message": error.to_string(),
                "details": query_plan_error_details(error),
            },
        }),
    )
}

fn plan_query_pack(
    args: &Args,
    pack: &QueryPack,
    refusal_out: &Path,
) -> BenchResult<(QueryInputPolicy, NlPlanConfig, BTreeMap<String, QueryPlan>)> {
    let policy_raw = required(args, "query-input-policy")?;
    let policy = match QueryInputPolicy::parse(&policy_raw) {
        Ok(policy) => policy,
        Err(error) => {
            write_query_plan_refusal(refusal_out, &policy_raw, None, None, None, &error)?;
            return Err(BenchError::Config(format!("--query-input-policy: {error}")));
        }
    };
    let config = nl_plan_config(args, policy)?;
    let execution_profile_sha256 = execution_profile_sha256(policy, &config);
    let mut plans = BTreeMap::new();
    for task in &pack.tasks {
        let plan = match plan_query(policy, task.query.as_str(), &config) {
            Ok(plan) => plan,
            Err(error) => {
                write_query_plan_refusal(
                    refusal_out,
                    &policy_raw,
                    Some(&execution_profile_sha256),
                    Some(task.task_id.as_str()),
                    Some(sha256_hex(task.query.as_bytes()).as_str()),
                    &error,
                )?;
                return Err(BenchError::Config(format!(
                    "query {} cannot be planned under policy {}: {error}",
                    task.task_id, policy_raw
                )));
            }
        };
        if plans.insert(task.task_id.clone(), plan).is_some() {
            return Err(BenchError::Protocol(format!(
                "duplicate query task ID during planning: {}",
                task.task_id
            )));
        }
    }
    Ok((policy, config, plans))
}

fn nl_plan_config(args: &Args, policy: QueryInputPolicy) -> BenchResult<NlPlanConfig> {
    let mut config = NlPlanConfig::default();
    if let Some(raw) = args.flags.get("nl-max-tokens") {
        if !matches!(
            policy,
            QueryInputPolicy::NaturalLanguage | QueryInputPolicy::NaturalLanguageFile
        ) {
            return Err(usage_error(
                "--nl-max-tokens requires a natural_language policy".to_string(),
            ));
        }
        if raw.is_empty() || !raw.bytes().all(|byte| byte.is_ascii_digit()) {
            return Err(usage_error(
                "--nl-max-tokens must be a decimal integer from 1 to 128".to_string(),
            ));
        }
        let value = raw.parse::<usize>().map_err(|_| {
            usage_error("--nl-max-tokens must be a decimal integer from 1 to 128".to_string())
        })?;
        if !(1..=128).contains(&value) {
            return Err(usage_error(
                "--nl-max-tokens must be a decimal integer from 1 to 128".to_string(),
            ));
        }
        config.max_tokens = value;
    }
    Ok(config)
}

fn validate_policy_routes(policy: QueryInputPolicy, routes: &BTreeSet<&str>) -> BenchResult<()> {
    if policy == QueryInputPolicy::ExactSymbolName && routes != &BTreeSet::from(["symbol"]) {
        return Err(BenchError::Config(
            "exact_symbol_name requires only the symbol route".to_string(),
        ));
    }
    if matches!(
        policy,
        QueryInputPolicy::LiteralFile
            | QueryInputPolicy::KeywordFile
            | QueryInputPolicy::SubstringFile
            | QueryInputPolicy::CodeSearchFile
            | QueryInputPolicy::CodeSearchExactContentFile
            | QueryInputPolicy::CodeSearchTypoFile
            | QueryInputPolicy::CodeSearchComponentsFile
            | QueryInputPolicy::NaturalLanguageFile
    ) && routes != &BTreeSet::from(["lexical"])
    {
        return Err(BenchError::Config(format!(
            "{} requires only the lexical route",
            policy.as_str()
        )));
    }
    Ok(())
}

fn validate_source_revision_for_policy(
    policy: QueryInputPolicy,
    revision_id: &str,
    repository_commit: &str,
) -> BenchResult<()> {
    if policy == QueryInputPolicy::CodeSearchExactContentFile && revision_id != repository_commit {
        return Err(BenchError::Protocol(
            "exact-content source revision must equal the pinned manifest repository commit"
                .to_string(),
        ));
    }
    Ok(())
}

fn require_external_path(repo: &Path, path: &Path, label: &str) -> BenchResult<PathBuf> {
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
    let name = path
        .file_name()
        .ok_or_else(|| BenchError::Config(format!("{label} has no file name")))?;
    Ok(canonical_parent.join(name))
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

fn symbol_preflight_options(
    args: &Args,
    max_file_bytes: u64,
) -> BenchResult<SymbolPreflightOptions<'static>> {
    Ok(SymbolPreflightOptions {
        max_file_bytes: usize::try_from(max_file_bytes).map_err(|error| {
            BenchError::Config(format!("max-file-bytes exceeds usize: {error}"))
        })?,
        max_symbols_per_file: optional_usize(args, "max-symbols-per-file", 100_000)?,
        max_symbols_total: optional_usize(args, "max-symbols-total", 1_000_000)?,
        timeout_per_file: Duration::from_millis(optional_u64(args, "symbol-timeout-ms", 10_000)?),
        timeout_total: Duration::from_millis(optional_u64(
            args,
            "symbol-total-timeout-ms",
            120_000,
        )?),
        max_diagnostics_per_file: optional_usize(args, "max-symbol-diagnostics-per-file", 32)?,
        max_diagnostics_total: optional_usize(args, "max-symbol-diagnostics", 1024)?,
        cancellation: None,
    })
}

fn run_symbol_preflight(args: &Args) -> BenchResult<()> {
    reject_unknown(
        args,
        &[
            "repo",
            "manifest",
            "out",
            "max-file-bytes",
            "symbol-coverage",
            "symbol-timeout-ms",
            "max-symbols-per-file",
            "max-symbols-total",
            "symbol-total-timeout-ms",
            "max-symbol-diagnostics-per-file",
            "max-symbol-diagnostics",
        ],
    )?;
    let policy_text = args
        .flags
        .get("symbol-coverage")
        .map_or("require-complete", String::as_str);
    let policy = SymbolCoveragePolicy::parse(policy_text)?;
    let repo = PathBuf::from(required(args, "repo")?);
    let out = PathBuf::from(required(args, "out")?);
    let _external_path = require_external_path(&repo, &out, "--out")?;
    if out.exists() {
        return Err(BenchError::Config(format!(
            "--out already exists: {}",
            out.display()
        )));
    }
    let manifest = load_manifest(&PathBuf::from(required(args, "manifest")?))?;
    verify_checkout(&repo, &manifest)?;
    let limits = CorpusLimits {
        max_file_bytes: optional_u64(
            args,
            "max-file-bytes",
            CorpusLimits::default().max_file_bytes,
        )?,
    };
    let options = symbol_preflight_options(args, limits.max_file_bytes)?;
    let files = load_corpus(&repo, &manifest, &limits)?;
    let by_path = files
        .into_iter()
        .map(|file| (file.path.clone(), file))
        .collect();
    let preflight = preflight_corpus_symbols(&by_path, &options)?;
    let value = serde_json::json!({
        "symbol_coverage_policy": policy_text,
        "repository_commit": manifest.repository_commit,
        "file_universe_sha256": quanta_index_retrieval_bench::corpus::universe_digest(&manifest.files),
        "preflight": preflight.report(),
    });
    // Preserve the complete census even when the selected policy refuses it.
    write_json(&out, &value)?;
    preflight.admit(policy)
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
    let _external_path = require_external_path(&repo, &out, "--out")?;
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
            "query-input-policy",
            "nl-max-tokens",
            "refusal-out",
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
            "diagnostics-out",
            "rank-study-out",
            "rank-study-max-files",
            "rank-study-max-pages",
            "rank-study-timeout-ms",
            "query-stage-observation",
            "experimental-hybrid-fetch-floor",
            "out",
            "io-timeout-secs",
            "ready-timeout-secs",
            "materialized-corpus-sha256",
            "model-dir",
            "symbol-coverage",
            "symbol-preflight-out",
            "symbol-timeout-ms",
            "symbol-total-timeout-ms",
            "max-symbols-per-file",
            "max-symbols-total",
            "max-symbol-diagnostics-per-file",
            "max-symbol-diagnostics",
            "source-stream-id",
            "source-event-id",
            "source-base-event-id",
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
    let state_root = PathBuf::from(required(args, "state-root")?);
    let _external_path = require_external_path(&repo, &state_root, "--state-root")?;
    let out = PathBuf::from(required(args, "out")?);
    let _external_path = require_external_path(&repo, &out, "--out")?;
    if out.exists() {
        return Err(BenchError::Config(format!(
            "--out already exists: {}",
            out.display()
        )));
    }
    let metrics_out = args.flags.get("metrics-out").map(PathBuf::from);
    if let Some(path) = &metrics_out {
        let _external_path = require_external_path(&repo, path, "--metrics-out")?;
        if path.exists() {
            return Err(BenchError::Config(format!(
                "--metrics-out already exists: {}",
                path.display()
            )));
        }
    }
    let diagnostics_out = args.flags.get("diagnostics-out").map(PathBuf::from);
    if let Some(path) = &diagnostics_out {
        let _external_path = require_external_path(&repo, path, "--diagnostics-out")?;
        if path.exists() {
            return Err(BenchError::Config(format!(
                "--diagnostics-out already exists: {}",
                path.display()
            )));
        }
    }
    let rank_study_out = args.flags.get("rank-study-out").map(PathBuf::from);
    if let Some(path) = &rank_study_out {
        let _external_path = require_external_path(&repo, path, "--rank-study-out")?;
        if path.exists() {
            return Err(BenchError::Config(format!(
                "--rank-study-out already exists: {}",
                path.display()
            )));
        }
    } else if [
        "rank-study-max-files",
        "rank-study-max-pages",
        "rank-study-timeout-ms",
    ]
    .iter()
    .any(|key| args.flags.contains_key(*key))
    {
        return Err(BenchError::Config(
            "rank-study limits require --rank-study-out".into(),
        ));
    }
    let rank_study_limits = rank_study::Limits {
        max_files: optional_usize(args, "rank-study-max-files", 10_000)?,
        max_pages: optional_usize(args, "rank-study-max-pages", 1_000)?,
        timeout: Duration::from_millis(optional_u64(args, "rank-study-timeout-ms", 30_000)?),
    };
    if !rank_study_limits.valid() {
        return Err(BenchError::Config(
            "rank-study limits require 1..100000 files, 1..10000 pages and 1..300000 ms".into(),
        ));
    }
    let refusal_out = PathBuf::from(required(args, "refusal-out")?);
    let _external_path = require_external_path(&repo, &refusal_out, "--refusal-out")?;
    if refusal_out.exists() {
        return Err(BenchError::Config(format!(
            "--refusal-out already exists: {}",
            refusal_out.display()
        )));
    }
    let symbol_preflight_out = args.flags.get("symbol-preflight-out").map_or_else(
        || {
            let mut path = out.as_os_str().to_os_string();
            path.push(".symbol-preflight.json");
            PathBuf::from(path)
        },
        PathBuf::from,
    );
    let _external_path =
        require_external_path(&repo, &symbol_preflight_out, "--symbol-preflight-out")?;
    if symbol_preflight_out.exists() {
        return Err(BenchError::Config(format!(
            "symbol preflight evidence already exists: {}",
            symbol_preflight_out.display()
        )));
    }
    let symbol_policy_text = args
        .flags
        .get("symbol-coverage")
        .map_or("require-complete", String::as_str);
    let symbol_policy = SymbolCoveragePolicy::parse(symbol_policy_text)?;
    // Caller-provided producer/run identity, independent of target generation.
    let source_event = quanta_index_contract::SourcePublicationEvent {
        stream_id: args
            .flags
            .get("source-stream-id")
            .cloned()
            .map_or_else(|| required(args, "runner-name"), Ok)?,
        event_id: args
            .flags
            .get("source-event-id")
            .cloned()
            .map_or_else(|| required(args, "run-id"), Ok)?,
        expected_base_event_id: args.flags.get("source-base-event-id").cloned(),
        payload_sha256: [0; 32], // SDK finalization hashes the complete logical body.
    };
    source_event
        .validate()
        .map_err(|error| BenchError::Config(error.to_string()))?;
    let mut output_paths = BTreeSet::new();
    for (label, path) in [
        ("--state-root", Some(&state_root)),
        ("--out", Some(&out)),
        ("--metrics-out", metrics_out.as_ref()),
        ("--diagnostics-out", diagnostics_out.as_ref()),
        ("--rank-study-out", rank_study_out.as_ref()),
        ("--refusal-out", Some(&refusal_out)),
        ("--symbol-preflight-out", Some(&symbol_preflight_out)),
    ] {
        if let Some(path) = path
            && !output_paths.insert(require_external_path(&repo, path, label)?)
        {
            return Err(BenchError::Config(format!(
                "{label} must differ from every other output/state path after resolving its parent"
            )));
        }
    }
    if let Some(path) = &metrics_out
        && require_external_path(&repo, path, "--metrics-out")?.parent()
            != require_external_path(&repo, &symbol_preflight_out, "--symbol-preflight-out")?
                .parent()
    {
        return Err(BenchError::Config(
            "symbol preflight and phase metrics must share an evidence directory".to_string(),
        ));
    }
    let symbol_preflight_ref = symbol_preflight_out
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| BenchError::Config("preflight artifact name is not UTF-8".to_string()))?;
    // The query plan is a preflight contract. No corpus chunking, daemon
    // boot, publication, or measured request may happen before every task
    // has one accepted plan.
    let (policy, nl_plan_config, task_plans) = plan_query_pack(args, &pack, &refusal_out)?;
    validate_policy_routes(policy, &selected)?;
    if rank_study_out.is_some()
        && (!rank_study::allowed(policy) || selected != BTreeSet::from(["lexical"]))
    {
        return Err(BenchError::Config(
            "rank study requires lexical-only ordinary CodeSearch file policy".into(),
        ));
    }
    validate_source_revision_for_policy(
        policy,
        &required(args, "revision-id")?,
        &manifest.repository_commit,
    )?;
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
    let symbol_preflight_start = Instant::now();
    let symbol_preflight = preflight_corpus_symbols(
        &by_path,
        &symbol_preflight_options(args, limits.max_file_bytes)?,
    )?;
    let symbol_preflight_sha256 = write_json_bound(
        &symbol_preflight_out,
        &serde_json::json!({
            "symbol_coverage_policy": symbol_policy_text,
            "repository_commit": manifest.repository_commit,
            "file_universe_sha256": quanta_index_retrieval_bench::corpus::universe_digest(&manifest.files),
            "preflight": symbol_preflight.report(),
        }),
    )?;
    symbol_preflight.admit(symbol_policy)?;
    let symbol_preflight_elapsed = symbol_preflight_start.elapsed();

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
    let symbol_coverage = symbol_preflight
        .report()
        .files
        .iter()
        .map(|file| {
            let count = match file.coverage {
                quanta_index_contract::SymbolCoverage::Complete { symbol_count } => {
                    Some(symbol_count)
                }
                quanta_index_contract::SymbolCoverage::NotRequested
                | quanta_index_contract::SymbolCoverage::Unsupported
                | quanta_index_contract::SymbolCoverage::ParseFailed
                | quanta_index_contract::SymbolCoverage::ProducerFailed => None,
            };
            serde_json::json!({
                "path": file.path, "source_sha256": file.source_sha256, "language": file.language,
                "definition_count": count, "coverage": file.coverage, "failure": file.failure,
            })
        })
        .collect::<Vec<_>>();
    let (batch, assembly) = assemble_batch(
        &identity,
        &selection.chunks,
        &by_path,
        &symbol_preflight,
        symbol_policy,
        source_event,
        selected.contains("semantic") || selected.contains("hybrid"),
    )?;
    let published_units = PublishedUnitRegistry::from_chunks_and_symbols(
        &selection.chunks,
        symbol_preflight.symbols(),
        &by_path,
    )?;

    let searchd_bin =
        resolve_searchd_binary(args.flags.get("searchd-bin").map(PathBuf::from).as_deref())?;
    let searchd_digest =
        verify_searchd_digest(&searchd_bin, &required(args, "searchd-expected-sha256")?)?;
    let profile = EmbedderProfile::resolve(args.flags.get("embedder").map(String::as_str))?;
    let model_dir = args.flags.get("model-dir").map(PathBuf::from);
    let query_stage_observation = QueryStageObservationPolicy::parse(
        args.flags
            .get("query-stage-observation")
            .map_or("enabled", String::as_str),
    )
    .map_err(|message| BenchError::Config(message.to_string()))?;
    let hybrid_fetch_floor = HybridFetchFloorPolicy::parse(
        args.flags
            .get("experimental-hybrid-fetch-floor")
            .map_or("100", String::as_str),
    )
    .map_err(|message| BenchError::Config(message.to_string()))?;
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
        query_stage_observation,
        hybrid_fetch_floor,
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
    let (receipt, ack, ingest_observation) =
        publish_and_activate(&session, &batch, &identity, None)?;
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
    let execution_profile = execution_profile_value(policy, &nl_plan_config);
    let execution_profile_digest = execution_profile_sha256(policy, &nl_plan_config);
    let mut provenance: BTreeMap<String, RouteProvenance> = BTreeMap::new();
    let mut captures: BTreeMap<String, CaptureProvenance> = BTreeMap::new();
    for route in routes.iter().copied() {
        let (model, model_revision) = if route == "lexical" {
            ("none:lexical", "not-applicable")
        } else if route == "symbol" {
            // Symbol search runs in the lexical domain: no embedding model
            // is exercised, so the capture must not claim one.
            ("none:symbol", "not-applicable")
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
                    source_repo_id: identity.repo_id.as_str().to_string(),
                    source_revision_id: identity.revision_id.as_str().to_string(),
                    receipt_digest: receipt_binding.clone(),
                    activation_digest: activation_binding.clone(),
                    model: model.to_string(),
                    model_revision: model_revision.to_string(),
                    execution_profile: execution_profile.clone(),
                    execution_profile_sha256: execution_profile_digest.clone(),
                },
            )
            .is_some()
        {
            return Err(BenchError::Protocol(format!(
                "duplicate capture_id: {capture_id}"
            )));
        }
    }
    let mut outcomes: BTreeMap<(String, String), QueryOutcome> = BTreeMap::new();
    let mut warm_latencies_ms: BTreeMap<String, BTreeMap<String, Vec<f64>>> = BTreeMap::new();
    let mut cold_latencies_ms: BTreeMap<String, f64> = BTreeMap::new();
    let mut query_observations = Vec::new();
    let mut completed_results = BTreeMap::new();
    let mut completed_query = |task_id: &str,
                               route: &'static str,
                               plan: &QueryPlan,
                               phase: &str,
                               iteration: usize|
     -> BenchResult<QueryOutcome> {
        let start = overall.elapsed();
        let query = RouteQuery {
            client: session.client(),
            route,
            lexical_request: &plan.lexical_request,
            semantic_text: &plan.semantic_text,
            repo_id: &identity.repo_id,
            revision_id: &identity.revision_id,
            generation: identity.generation,
            top_k,
        };
        let mut outcome = query_route_with_policy(&query, plan.policy);
        let mut row = result_value(
            task_id,
            query.route,
            &outcome,
            plan,
            top_k,
            &by_path,
            &published_units,
        )?;
        let status = row
            .get("status")
            .and_then(serde_json::Value::as_str)
            .ok_or_else(|| BenchError::Protocol("completed response lacks status".to_string()))?
            .to_string();
        let object = row.as_object_mut().ok_or_else(|| {
            BenchError::Protocol("completed response is not an object".to_string())
        })?;
        let _timing = object.remove("timings");
        let output = serde_json::to_vec(&row).map_err(|error| {
            BenchError::Protocol(format!("completed response cannot serialize: {error}"))
        })?;
        let end = overall.elapsed();
        let elapsed = end.saturating_sub(start);
        match &mut outcome {
            QueryOutcome::ReturnedWindow { latency, .. }
            | QueryOutcome::RejectedResponse { latency, .. }
            | QueryOutcome::SdkFailure { latency, .. } => *latency = elapsed,
        }
        let start_ns = u64::try_from(start.as_nanos()).map_err(|error| {
            BenchError::Protocol(format!("query start clock overflow: {error}"))
        })?;
        let end_ns = u64::try_from(end.as_nanos())
            .map_err(|error| BenchError::Protocol(format!("query end clock overflow: {error}")))?;
        query_observations.push(serde_json::json!({
            "task_id": task_id, "route": query.route, "phase": phase,
            "iteration": iteration, "start_ns": start_ns, "end_ns": end_ns,
            "status": status, "output_bytes": output.len(),
        }));
        if phase == "measured" && iteration == 0 {
            let _previous_timing = row
                .as_object_mut()
                .ok_or_else(|| {
                    BenchError::Protocol("completed response is not an object".to_string())
                })?
                .insert(
                    "timings".to_string(),
                    serde_json::json!({"query_latency_ms": elapsed.as_secs_f64() * 1000.0}),
                );
            if completed_results
                .insert((task_id.to_string(), route.to_string()), row)
                .is_some()
            {
                return Err(BenchError::Protocol(
                    "duplicate completed response".to_string(),
                ));
            }
        }
        Ok(outcome)
    };
    let mut warmup_elapsed = Duration::ZERO;
    let first_query_elapsed;
    let warm_query_elapsed = if let Some(protocol) = &query_protocol {
        let cold_start = Instant::now();
        let cold_plan = task_plans
            .get(protocol.cold_probe_task_id.as_str())
            .ok_or_else(|| BenchError::Protocol("cold probe task disappeared".to_string()))?;
        for route in routes.iter().copied() {
            let outcome =
                completed_query(&protocol.cold_probe_task_id, route, cold_plan, "cold", 0)?;
            let latency = match &outcome {
                QueryOutcome::ReturnedWindow { latency, .. } => *latency,
                failed @ (QueryOutcome::RejectedResponse { .. }
                | QueryOutcome::SdkFailure { .. }) => {
                    let classification = failed.classification().map_err(|message| {
                        BenchError::Protocol(format!("invalid cold outcome: {message}"))
                    })?;
                    return Err(BenchError::Protocol(format!(
                        "cold probe failed for route {route}: {}/{}",
                        classification.status,
                        classification
                            .error_code
                            .as_deref()
                            .unwrap_or("missing_error_code")
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
        for (iteration, schedule) in protocol.warmup_schedules.iter().enumerate() {
            for task_id in schedule {
                let plan = task_plans
                    .get(task_id.as_str())
                    .ok_or_else(|| BenchError::Protocol("warmup task disappeared".to_string()))?;
                for route in routes.iter().copied() {
                    let outcome = completed_query(task_id, route, plan, "warmup", iteration)?;
                    let _latency = validated_protocol_latency(&outcome, "warmup")?;
                }
            }
        }
        warmup_elapsed = warmup_start.elapsed();

        let measurement_start = Instant::now();
        for (repetition, schedule) in protocol.measurement_schedules.iter().enumerate() {
            for task_id in schedule {
                let plan = task_plans.get(task_id.as_str()).ok_or_else(|| {
                    BenchError::Protocol("measurement task disappeared".to_string())
                })?;
                for route in routes.iter().copied() {
                    let outcome = completed_query(task_id, route, plan, "measured", repetition)?;
                    let latency = validated_protocol_latency(&outcome, "measurement")?;
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
        measurement_start.elapsed()
    } else {
        let query_start = Instant::now();
        let mut first = Duration::ZERO;
        for task in &pack.tasks {
            let plan = task_plans
                .get(task.task_id.as_str())
                .ok_or_else(|| BenchError::Protocol("planned task disappeared".to_string()))?;
            for route in routes.iter().copied() {
                let single_query_start = Instant::now();
                let outcome = completed_query(&task.task_id, route, plan, "measured", 0)?;
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
        query_start.elapsed().saturating_sub(first)
    };

    let record_start = Instant::now();
    let record = runner_record(&RunnerRecordInput {
        pack: &pack,
        identity: &identity_block,
        provenance: &provenance,
        captures: &captures,
        outcomes: &outcomes,
        completed_results: &completed_results,
        plans: &task_plans,
        nl_config: &nl_plan_config,
        top_k,
    })?;
    let native_spans = if diagnostics_out.is_some() {
        quanta_index_retrieval_bench::record::native_span_proofs(
            &outcomes,
            &by_path,
            &published_units,
        )?
    } else {
        BTreeMap::new()
    };
    let record_elapsed = record_start.elapsed();
    // Optional diagnostics run only after every measured request. Their refusals
    // are artifact rows, not capture failures or zero-quality replacements.
    let rank_study_start = Instant::now();
    let rank_study_rows = rank_study_out.as_ref().map(|_path| {
        rank_study::collect(
            session.client(),
            &quanta_index_contract::GenerationPin::new(
                identity.repo_id.clone(),
                identity.revision_id.clone(),
                identity.generation,
            ),
            &pack,
            &task_plans,
            &outcomes,
            top_k,
            rank_study_limits,
        )
    });
    let rank_study_elapsed = rank_study_start.elapsed();
    let verify_start = Instant::now();
    verify_capture_corpus(args, &repo, &manifest)?;
    let verify_elapsed = verify_start.elapsed();
    let binary = session.searchd_binary().display().to_string();
    let shutdown_start = Instant::now();
    session.stop()?;
    let shutdown_elapsed = shutdown_start.elapsed();
    let overall_elapsed = overall.elapsed();
    let phase_sum = discovery_elapsed
        .checked_add(symbol_preflight_elapsed)
        .and_then(|value| value.checked_add(chunk_elapsed))
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
    let diagnostics = if diagnostics_out.is_some() {
        let mut value = diagnostic_value(
            &record_digest,
            &record,
            &pack,
            &routes,
            &outcomes,
            query_stage_observation,
            hybrid_fetch_floor,
            &native_spans,
        )?;
        let detail = serde_json::json!({
            "clock": "runner_monotonic_wall_v1",
            "daemon_boot_and_readiness": boot_elapsed.as_secs_f64() * 1000.0,
            "sdk_publish_and_activate_opaque": publish_elapsed.as_secs_f64() * 1000.0,
            "runner_record_assembly": record_elapsed.as_secs_f64() * 1000.0,
            "corpus_reverification": verify_elapsed.as_secs_f64() * 1000.0,
            "daemon_shutdown": shutdown_elapsed.as_secs_f64() * 1000.0,
        });
        let object = value.as_object_mut().ok_or_else(|| {
            BenchError::Protocol("diagnostic value must be an object".to_string())
        })?;
        let _previous = object.insert(
            "ingest".to_string(),
            serde_json::json!({
                "receipt": receipt,
                "activation_ack": ack,
                "observation": ingest_observation,
            }),
        );
        if object
            .insert("runner_timing_detail_ms".to_string(), detail)
            .is_some()
        {
            return Err(BenchError::Protocol(
                "duplicate diagnostic timing detail".to_string(),
            ));
        }
        Some(value)
    } else {
        None
    };
    let mut phase_metrics = serde_json::json!({
        "schema_version": 2,
        "system": "quanta",
        "timing_layer": "runner_monotonic_wall_v1",
        "query_timing": {
            "boundary": "request_construction_to_normalized_response",
            "clock": "capture_relative_monotonic_ns",
            "observations": query_observations,
        },
        "strategy": selection.name,
        "record_sha256": record_digest,
        "runner_binary_sha256": runner_digest,
        "task_count": pack.tasks.len(),
        "route_count": routes.len(),
        "file_count": selection.coverage.files,
        "chunk_count": selection.coverage.chunks,
        "symbol_count": assembly.symbols,
        "symbol_producer_identity": quanta_index_retrieval_bench::symbols::SYMBOL_PRODUCER_IDENTITY,
        "symbol_grammars": quanta_index_retrieval_bench::symbols::SYMBOL_PRODUCER_GRAMMARS,
        "symbol_coverage": symbol_coverage,
        "symbol_coverage_policy": symbol_policy_text,
        "symbol_preflight_out": symbol_preflight_ref,
        "symbol_preflight_sha256": symbol_preflight_sha256,
        "symbol_producer_policy_sha256": symbol_preflight.report().producer_policy_sha256,
        "symbol_incomplete_files": symbol_preflight.report().incomplete_files,
        "symbol_unsupported_files": symbol_preflight.report().files.iter().filter(|file| file.coverage == quanta_index_contract::SymbolCoverage::Unsupported).count(),
        "symbol_unsupported_details": symbol_preflight.report().files.iter().filter(|file| file.coverage == quanta_index_contract::SymbolCoverage::Unsupported).collect::<Vec<_>>(),
        "empty_scopes": assembly.empty_scopes.len(),
        "symbol_only_scopes": assembly.symbol_only_scopes.len(),
        "query_schedule": pack.tasks.iter().map(|task| task.task_id.as_str()).collect::<Vec<_>>(),
        "warmup_passes": query_protocol.as_ref().map_or(0, |value| value.warmup_schedules.len()),
        "measurement_repetitions": query_protocol.as_ref().map_or(1, |value| value.measurement_schedules.len()),
        "phases_ms": {
            "discovery": discovery_elapsed.as_secs_f64() * 1000.0,
            "symbol_preflight": symbol_preflight_elapsed.as_secs_f64() * 1000.0,
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
    if let (Some(path), Some(value)) = (&diagnostics_out, &diagnostics) {
        write_json(path, value)?;
    }
    if let (Some(path), Some(results)) = (&rank_study_out, rank_study_rows) {
        write_json(
            path,
            &serde_json::json!({
                "schema_version":1,"kind":"quanta_code_search_rank_study",
                "qualification":"diagnostic_unqualified","record_sha256":record_digest,
                "policy":policy.as_str(),"execution_profile_sha256":execution_profile_sha256(policy, &nl_plan_config),
                "timing_boundary":"post_measurement_sdk_paging_and_explanations",
                "diagnostic_ms":rank_study_elapsed.as_secs_f64()*1000.0,
                "limits":{"max_files":rank_study_limits.max_files,"max_pages":rank_study_limits.max_pages,
                    "timeout_ms":rank_study_limits.timeout.as_millis()},
                "results":results,
            }),
        )?;
    }
    // A failed owned-daemon shutdown or phase-artifact write must not leave a
    // scoreable success record. The record is the final create-new artifact.
    write_json(&out, &record)?;

    let mut status_counts: BTreeMap<&str, usize> = BTreeMap::new();
    for outcome in outcomes.values() {
        let status = match outcome {
            QueryOutcome::ReturnedWindow { .. } => "returned_window",
            QueryOutcome::RejectedResponse { .. } => "rejected_response",
            QueryOutcome::SdkFailure { status, .. } => status,
        };
        let count = status_counts.entry(status).or_insert(0);
        *count = count
            .checked_add(1)
            .ok_or_else(|| BenchError::Protocol("outcome status count overflow".to_string()))?;
    }
    stdout_line(&format!(
        "captured {} tasks x {} routes via {} under query_input_policy={} (receipt gen {}, ack active {:?}); chunk={}ms boot={}ms publish={}ms query={}ms total={}ms; outcomes={status_counts:?}; binary={binary}",
        pack.tasks.len(),
        routes.len(),
        selection.name,
        policy.as_str(),
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
            "expected exactly one subcommand: run|chunk|preflight".to_string(),
        ));
    }
    match parsed.positional.first().map(String::as_str) {
        Some("run") => run_capture(&parsed),
        Some("chunk") => run_chunk(&parsed),
        Some("preflight") => run_symbol_preflight(&parsed),
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
    fn protocol_preserves_typed_query_refusal_and_rejects_malformed_failure() {
        let latency = Duration::from_millis(7);
        let refused = QueryOutcome::SdkFailure {
            status: "error",
            code: "LEX_TRIGRAM_PLAN_LIMIT_EXCEEDED".into(),
            message: "candidate plan exceeded limit".into(),
            latency,
        };
        assert_eq!(
            validated_protocol_latency(&refused, "warmup").expect("typed warmup refusal"),
            latency
        );
        assert_eq!(
            validated_protocol_latency(&refused, "measurement").expect("typed measurement refusal"),
            latency
        );
        let malformed = QueryOutcome::SdkFailure {
            status: "success",
            code: String::new(),
            message: String::new(),
            latency,
        };
        assert!(validated_protocol_latency(&malformed, "warmup").is_err());
    }

    #[test]
    fn exact_content_source_revision_must_match_manifest_commit() {
        let commit = "a".repeat(40);
        assert!(
            validate_source_revision_for_policy(
                QueryInputPolicy::CodeSearchExactContentFile,
                &commit,
                &commit
            )
            .is_ok()
        );
        assert!(
            validate_source_revision_for_policy(
                QueryInputPolicy::CodeSearchExactContentFile,
                "synthetic-revision",
                &commit
            )
            .is_err()
        );
        assert!(
            validate_source_revision_for_policy(
                QueryInputPolicy::CodeSearchFile,
                "synthetic-revision",
                &commit
            )
            .is_ok()
        );
    }

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
        let digest =
            write_json_bound(&output, &serde_json::json!({"run": 1})).expect("first write");
        assert_eq!(
            digest,
            sha256_hex(&std::fs::read(&output).expect("exact artifact bytes"))
        );
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

    #[test]
    fn query_plan_preflight_writes_one_create_new_refusal() {
        let parent = tempfile::tempdir().expect("tempdir");
        let refusal = parent.path().join("query-plan-refusal.json");
        let args = Args {
            positional: vec!["run".to_string()],
            flags: BTreeMap::from([(
                "query-input-policy".to_string(),
                "natural_language".to_string(),
            )]),
            help: false,
        };
        let pack = QueryPack {
            suite_id: "suite".to_string(),
            suite_commitment_sha256: "a".repeat(64),
            repository_commit: "b".repeat(40),
            tokenizer: "test".to_string(),
            tokenizer_budget_version: None,
            routes: vec!["lexical".to_string()],
            file_universe: vec![("src/lib.rs".to_string(), "c".repeat(64))],
            file_universe_digest: "d".repeat(64),
            tasks: vec![quanta_index_retrieval_bench::record::PackTask {
                task_id: "T01".to_string(),
                query: "---".to_string(),
                query_sha256: sha256_hex(b"---"),
            }],
            pack_sha256: "e".repeat(64),
            comparison_contract: serde_json::json!({"top_k": 10}),
            contract_top_k: 10,
        };

        let error = plan_query_pack(&args, &pack, &refusal)
            .expect_err("unindexable task must refuse during preflight");
        assert!(error.to_string().contains("T01"));
        let artifact: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&refusal).expect("refusal bytes"))
                .expect("refusal JSON");
        let artifact = artifact.as_object().expect("refusal artifact is an object");
        assert_eq!(artifact.get("schema_version").expect("schema version"), 1);
        assert_eq!(artifact.get("phase").expect("phase"), "query_plan");
        assert_eq!(artifact.get("task_id").expect("task id"), "T01");
        let error = artifact
            .get("error")
            .and_then(serde_json::Value::as_object)
            .expect("typed refusal error");
        assert_eq!(
            error.get("code").expect("error code"),
            "RBR_QUERY_NO_INDEXABLE_TOKENS"
        );
        assert_eq!(
            artifact
                .get("original_query_sha256")
                .expect("original query digest"),
            &sha256_hex(b"---")
        );
        assert!(
            artifact
                .get("execution_profile_sha256")
                .and_then(serde_json::Value::as_str)
                .is_some()
        );

        assert!(plan_query_pack(&args, &pack, &refusal).is_err());
    }

    #[test]
    fn exact_symbol_name_policy_refuses_other_routes_before_capture() {
        let symbol = BTreeSet::from(["symbol"]);
        let lexical = BTreeSet::from(["lexical"]);
        let mixed = BTreeSet::from(["lexical", "symbol"]);
        assert!(validate_policy_routes(QueryInputPolicy::ExactSymbolName, &symbol).is_ok());
        for routes in [&lexical, &mixed] {
            assert!(
                validate_policy_routes(QueryInputPolicy::ExactSymbolName, routes).is_err_and(
                    |error| error.to_string().contains("requires only the symbol route")
                )
            );
        }
        assert!(validate_policy_routes(QueryInputPolicy::Native, &lexical).is_ok());
        for policy in [
            QueryInputPolicy::LiteralFile,
            QueryInputPolicy::KeywordFile,
            QueryInputPolicy::SubstringFile,
            QueryInputPolicy::CodeSearchFile,
            QueryInputPolicy::CodeSearchExactContentFile,
            QueryInputPolicy::CodeSearchTypoFile,
        ] {
            assert!(validate_policy_routes(policy, &lexical).is_ok());
        }
        for routes in [&symbol, &mixed] {
            assert!(validate_policy_routes(QueryInputPolicy::LiteralFile, routes).is_err());
            assert!(validate_policy_routes(QueryInputPolicy::KeywordFile, routes).is_err());
            assert!(validate_policy_routes(QueryInputPolicy::SubstringFile, routes).is_err());
            assert!(validate_policy_routes(QueryInputPolicy::CodeSearchFile, routes).is_err());
            assert!(
                validate_policy_routes(QueryInputPolicy::CodeSearchExactContentFile, routes)
                    .is_err()
            );
            assert!(validate_policy_routes(QueryInputPolicy::CodeSearchTypoFile, routes).is_err());
        }
    }
}
