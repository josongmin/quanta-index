#![forbid(unsafe_code)]
#![deny(unused_must_use)]
#![deny(clippy::let_underscore_must_use)]
#![deny(clippy::map_err_ignore)]

use std::collections::VecDeque;
use std::fmt::Write as _;
use std::fs;
use std::io::{Read as _, Write};
use std::path::PathBuf;
use std::process::ExitCode;

use quanta_index_contract::{
    AuxEpochV1, ContinuationTokenV2, EarlyStopReason, EngineTouched, ExplainCandidateV1,
    GenerationPin, HistoryOrderV1, HistoryQueryRequest, HybridCandidateV1, HybridQueryRequest,
    HybridQueryResponse, HybridSeedQueryRequest, HybridSeedQueryResponse, LexicalCandidate,
    ManifestGeneration, PlannerTraceEntry, QueryConstraintSetV1, QueryErrorRepair,
    QueryResultWindowV2, RepoId, RepoMapDocType, RepoMapFocusSubjectDto, RepoMapQueryRequest,
    RevisionId, RuntimeMetadataQueryRequest, SearchExplanation, SearchPlaneHistoryQueryResponse,
    SearchPlaneQueryIpcResponse, SearchPlaneQueryIpcResponseEnvelope,
    SearchPlaneRuntimeMetadataQueryResponse, SearchPlaneStructuralQueryResponse,
    SemanticQueryRequest, StructuralQueryRequest, SymbolCandidate, SymbolQueryRequest,
    SymbolQueryResponse, TextQueryRequest, TextQueryResponse, TextQuerySyntax,
    ipc::{
        GenerationStatusReport, MetricHistogramV1, MetricsSnapshotV1, QuarantineDiscardAck,
        QuarantineDiscardOutcomeDtoV1, QuarantineInventoryV1, QuarantineTargetV1,
        QuarantinedGenerationEntryV1, QuarantinedRepoMapFileEntryV1, SearchPlaneTrackKind,
    },
};
use quanta_index_sdk::{ConnectOptions, QuantaIndex, SdkError};

const REQUEST_ID: u64 = 1;
const EXIT_TRANSPORT: u8 = 1;
const EXIT_USAGE: u8 = 2;
const EXIT_REMOTE: u8 = 3;
const EXIT_PROTOCOL: u8 = 4;

pub fn run<I, T>(args: I, stdout: &mut dyn Write, stderr: &mut dyn Write) -> ExitCode
where
    I: IntoIterator<Item = T>,
    T: Into<String>,
{
    match run_inner(args, stdout, stderr) {
        Ok(code) => code,
        Err(error) => {
            match writeln!(stderr, "error: {}", error.message) {
                Ok(()) | Err(_) => {}
            }
            if error.exit_code == EXIT_USAGE {
                match writeln!(stderr) {
                    Ok(()) | Err(_) => {}
                }
                match write!(stderr, "{}", usage()) {
                    Ok(()) | Err(_) => {}
                }
            }
            ExitCode::from(error.exit_code)
        }
    }
}

fn run_inner<I, T>(args: I, stdout: &mut dyn Write, _stderr: &mut dyn Write) -> CliResult<ExitCode>
where
    I: IntoIterator<Item = T>,
    T: Into<String>,
{
    let ParsedCommand {
        kind,
        output,
        connect_options,
        request,
    } = ParsedCommand::parse(args)?;
    let client = QuantaIndex::connect(connect_options).map_err(map_sdk_error)?;
    // J7Q-05: `readiness` and `doctor` dispatch via the CONTROL plane, not the
    // query plane. Branch them here so the query path (`dispatch_query_request`
    // -> `validate_response_kind` -> `render_response`) stays untouched.
    match request {
        CliRequest::Readiness {
            repo_id,
            revision_id,
        } => {
            let report = client
                .generations()
                .status(repo_id, revision_id)
                .map_err(map_sdk_error)?;
            write_stdout(stdout, &render_readiness(&report, output)?)?;
            Ok(ExitCode::SUCCESS)
        }
        CliRequest::Doctor {
            repo_id,
            revision_id,
        } => {
            let report = build_doctor_report(&client, repo_id, revision_id)?;
            write_stdout(stdout, &render_doctor(&report, output)?)?;
            Ok(ExitCode::SUCCESS)
        }
        CliRequest::Metrics => {
            let snapshot = client
                .observability()
                .metrics_snapshot()
                .map_err(map_sdk_error)?;
            write_stdout(stdout, &render_metrics(&snapshot, output)?)?;
            Ok(ExitCode::SUCCESS)
        }
        CliRequest::QuarantineList => {
            let inventory = client.quarantine().inventory().map_err(map_sdk_error)?;
            write_stdout(stdout, &render_quarantine_inventory(&inventory, output)?)?;
            Ok(ExitCode::SUCCESS)
        }
        CliRequest::QuarantineDiscard(target) => {
            let ack = client
                .quarantine()
                .discard(&target)
                .map_err(map_sdk_error)?;
            write_stdout(stdout, &render_quarantine_discard(&ack, output)?)?;
            Ok(ExitCode::SUCCESS)
        }
        query_request @ (CliRequest::Lexical(_)
        | CliRequest::Symbol(_)
        | CliRequest::Semantic(_)
        | CliRequest::Hybrid(_)
        | CliRequest::HybridSeed(_)
        | CliRequest::Explain { .. }
        | CliRequest::RepoMap(_)
        | CliRequest::RuntimeMetadata(_)
        | CliRequest::History(_)
        | CliRequest::Structural(_)) => {
            let response = dispatch_query_request(&client, query_request)?;
            validate_response_kind(kind, &response)?;
            render_response(output, &response, stdout)?;
            Ok(ExitCode::SUCCESS)
        }
    }
}

/// Write a fully-rendered CLI string to stdout.
///
/// Maps any I/O failure to a typed transport error. Shared by the control-plane
/// renderers (`readiness` / `doctor`) so the failure shape cannot drift.
fn write_stdout(stdout: &mut dyn Write, rendered: &str) -> CliResult<()> {
    stdout
        .write_all(rendered.as_bytes())
        .map_err(|err| CliError::transport(format!("failed writing stdout: {err}")))
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum OutputMode {
    Pretty,
    Json,
    /// Prometheus text exposition; only `metrics` renders it (QI-BB-015).
    Prometheus,
}

impl OutputMode {
    fn parse(value: &str) -> CliResult<Self> {
        match value {
            "pretty" => Ok(Self::Pretty),
            "json" => Ok(Self::Json),
            "prometheus" => Ok(Self::Prometheus),
            other => Err(CliError::usage(format!(
                "unsupported output mode `{other}`; expected `pretty`, `json` or `prometheus`"
            ))),
        }
    }
}

/// The typed refusal every non-metrics renderer gives `--output prometheus`.
///
/// `ParsedCommand::parse` refuses the combination before any socket is
/// touched; the renderers keep the arm so the refusal is the same wherever
/// the mode arrives.
fn prometheus_is_metrics_only(command: &str) -> CliError {
    CliError::usage(format!(
        "`--output prometheus` renders only `metrics`, not `{command}`"
    ))
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum CommandKind {
    Lexical,
    Symbol,
    Semantic,
    Hybrid,
    HybridSeed,
    Explain,
    RepoMap,
    RuntimeMetadata,
    History,
    Structural,
    Readiness,
    Doctor,
    Metrics,
    Quarantine,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct CommonOptions {
    output: OutputMode,
    socket_override: Option<PathBuf>,
    state_root_override: Option<PathBuf>,
}

impl Default for CommonOptions {
    fn default() -> Self {
        Self {
            output: OutputMode::Pretty,
            socket_override: None,
            state_root_override: None,
        }
    }
}

impl CommonOptions {
    fn parse_flag(&mut self, current: &str, rest: &mut VecDeque<String>) -> CliResult<bool> {
        match current {
            "--socket" => {
                self.socket_override = Some(PathBuf::from(take_value(rest, "--socket")?));
                Ok(true)
            }
            "--state-root" => {
                self.state_root_override = Some(PathBuf::from(take_value(rest, "--state-root")?));
                Ok(true)
            }
            "--output" => {
                self.output = OutputMode::parse(&take_value(rest, "--output")?)?;
                Ok(true)
            }
            "-h" | "--help" => Err(CliError::usage("help requested".to_string())),
            _ => Ok(false),
        }
    }

    fn resolve_connect_options(&self) -> ConnectOptions {
        match (&self.state_root_override, &self.socket_override) {
            (Some(state_root), Some(query_socket)) => {
                ConnectOptions::from_state_root(state_root.clone())
                    .with_query_socket(query_socket.clone())
            }
            (Some(state_root), None) => ConnectOptions::from_state_root(state_root.clone()),
            (None, Some(query_socket)) => ConnectOptions::default()
                .with_query_socket(query_socket.clone())
                .with_control_socket(query_socket.with_file_name("control.sock"))
                .with_ingest_socket(query_socket.with_file_name("ingest.sock")),
            (None, None) => ConnectOptions::default(),
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
enum CliRequest {
    Lexical(TextQueryRequest),
    Symbol(SymbolQueryRequest),
    Semantic(SemanticQueryRequest),
    /// QI-BB-018: two independent lanes fused by RRF.
    Hybrid(HybridQueryRequest),
    HybridSeed(HybridSeedQueryRequest),
    Explain {
        generation: GenerationPin,
        candidate: ExplainCandidateV1,
        text_query: Option<TextQueryRequest>,
        /// QI-BB-022: the dense query a hybrid row is re-derived under.
        semantic_query_text: Option<String>,
    },
    RepoMap(RepoMapQueryRequest),
    RuntimeMetadata(RuntimeMetadataQueryRequest),
    History(HistoryQueryRequest),
    Structural(StructuralQueryRequest),
    Readiness {
        repo_id: RepoId,
        revision_id: RevisionId,
    },
    Doctor {
        repo_id: RepoId,
        revision_id: RevisionId,
    },
    /// QI-BB-015: the daemon's metrics snapshot; takes no arguments.
    Metrics,
    /// QI-BB-026: what the daemon quarantines right now.
    QuarantineList,
    /// QI-BB-026: discard one quarantined entry exactly as listed.
    QuarantineDiscard(QuarantineTargetV1),
}

#[derive(Clone, Debug, PartialEq)]
struct ParsedCommand {
    kind: CommandKind,
    output: OutputMode,
    connect_options: ConnectOptions,
    request: CliRequest,
}

impl ParsedCommand {
    fn parse<I, T>(args: I) -> CliResult<Self>
    where
        I: IntoIterator<Item = T>,
        T: Into<String>,
    {
        let mut rest: VecDeque<String> = args.into_iter().map(Into::into).collect();
        if rest.is_empty() {
            return Err(CliError::usage("missing subcommand".to_string()));
        }
        let mut common = CommonOptions::default();
        let subcommand = loop {
            let current = rest
                .pop_front()
                .ok_or_else(|| CliError::usage("missing subcommand".to_string()))?;
            if common.parse_flag(&current, &mut rest)? {
                continue;
            }
            break current;
        };
        let (kind, payload) = match subcommand.as_str() {
            "lexical" => (CommandKind::Lexical, parse_lexical(&mut common, &mut rest)?),
            "symbol" => (CommandKind::Symbol, parse_symbol(&mut common, &mut rest)?),
            "semantic" => (
                CommandKind::Semantic,
                parse_semantic(&mut common, &mut rest)?,
            ),
            "hybrid" => (CommandKind::Hybrid, parse_hybrid(&mut common, &mut rest)?),
            "hybrid-seed" => (
                CommandKind::HybridSeed,
                parse_hybrid_seed(&mut common, &mut rest)?,
            ),
            "explain" => (CommandKind::Explain, parse_explain(&mut common, &mut rest)?),
            "repomap" | "repomap-query" => {
                (CommandKind::RepoMap, parse_repomap(&mut common, &mut rest)?)
            }
            "runtime-metadata" => (
                CommandKind::RuntimeMetadata,
                parse_runtime_metadata(&mut common, &mut rest)?,
            ),
            "history" => (CommandKind::History, parse_history(&mut common, &mut rest)?),
            "structural" => (
                CommandKind::Structural,
                parse_structural(&mut common, &mut rest)?,
            ),
            "readiness" => (
                CommandKind::Readiness,
                parse_readiness(&mut common, &mut rest)?,
            ),
            "doctor" => (CommandKind::Doctor, parse_doctor(&mut common, &mut rest)?),
            "metrics" => (CommandKind::Metrics, parse_metrics(&mut common, &mut rest)?),
            "quarantine" => (
                CommandKind::Quarantine,
                parse_quarantine(&mut common, &mut rest)?,
            ),
            other => {
                return Err(CliError::usage(format!(
                    "unknown subcommand `{other}`; expected lexical|symbol|semantic|hybrid|hybrid-seed|explain|repomap|runtime-metadata|history|structural|readiness|doctor|metrics|quarantine"
                )));
            }
        };
        if !rest.is_empty() {
            let extra = rest.pop_front().unwrap_or_default();
            return Err(CliError::usage(format!(
                "unexpected trailing argument `{extra}`"
            )));
        }
        if common.output == OutputMode::Prometheus && kind != CommandKind::Metrics {
            return Err(prometheus_is_metrics_only(&subcommand));
        }
        Ok(Self {
            kind,
            output: common.output,
            connect_options: common.resolve_connect_options(),
            request: payload,
        })
    }
}

#[derive(Default)]
struct PinnedGenerationArgs {
    repo_id: Option<String>,
    revision_id: Option<String>,
    manifest_generation: Option<u64>,
}

impl PinnedGenerationArgs {
    fn parse_flag(&mut self, current: &str, rest: &mut VecDeque<String>) -> CliResult<bool> {
        match current {
            "--repo-id" => {
                self.repo_id = Some(take_value(rest, "--repo-id")?);
                Ok(true)
            }
            "--revision-id" => {
                self.revision_id = Some(take_value(rest, "--revision-id")?);
                Ok(true)
            }
            "--manifest-generation" => {
                self.manifest_generation = Some(parse_u64_flag(rest, "--manifest-generation")?);
                Ok(true)
            }
            _ => Ok(false),
        }
    }

    fn into_generation_pin(self) -> CliResult<GenerationPin> {
        parse_generation_pin(self.repo_id, self.revision_id, self.manifest_generation)
    }
}

fn parse_query_command_flags(
    common: &mut CommonOptions,
    generation_args: &mut PinnedGenerationArgs,
    rest: &mut VecDeque<String>,
    command: &str,
    mut parse_local: impl FnMut(&str, &mut VecDeque<String>) -> CliResult<bool>,
) -> CliResult<()> {
    while let Some(current) = rest.pop_front() {
        if common.parse_flag(&current, rest)? {
            continue;
        }
        if generation_args.parse_flag(&current, rest)? {
            continue;
        }
        if parse_local(&current, rest)? {
            continue;
        }
        return Err(CliError::usage(format!(
            "unknown {command} flag `{current}`"
        )));
    }
    Ok(())
}

fn parse_lexical(common: &mut CommonOptions, rest: &mut VecDeque<String>) -> CliResult<CliRequest> {
    let page = parse_keyset_page_query(common, rest, "lexical")?;
    let cursor = page.cursor()?;
    Ok(CliRequest::Lexical(TextQueryRequest {
        cursor,
        ..page.text_query
    }))
}

fn parse_symbol(common: &mut CommonOptions, rest: &mut VecDeque<String>) -> CliResult<CliRequest> {
    let page = parse_keyset_page_query(common, rest, "symbol")?;
    let cursor = page.cursor()?;
    Ok(CliRequest::Symbol(SymbolQueryRequest::from(
        TextQueryRequest {
            cursor,
            ..page.text_query
        },
    )))
}

fn parse_runtime_metadata(
    common: &mut CommonOptions,
    rest: &mut VecDeque<String>,
) -> CliResult<CliRequest> {
    let page = parse_keyset_page_query(common, rest, "runtime-metadata")?;
    let cursor = page.cursor()?;
    Ok(CliRequest::RuntimeMetadata(RuntimeMetadataQueryRequest {
        text_query: page.text_query,
        cursor,
    }))
}

/// The flags of one keyset-paged query route: the text query and, for a
/// continuation, the cursor a previous page printed.
struct KeysetPageQueryArgs {
    text_query: TextQueryRequest,
    /// The `--cursor-json` path (or `-` for stdin) and the route it is for.
    cursor_json: Option<(String, &'static str)>,
}

impl KeysetPageQueryArgs {
    /// Parse only the opaque public token; the daemon owns route binding.
    /// The JSON is what `--output json` prints as `next_cursor`.
    fn cursor(&self) -> CliResult<Option<ContinuationTokenV2>> {
        self.cursor_json
            .as_ref()
            .map(|(path, command)| {
                let raw = read_json_text(path, &format!("{command} cursor"))?;
                serde_json::from_str::<ContinuationTokenV2>(&raw).map_err(|err| {
                    CliError::usage(format!(
                        "failed to decode {command} cursor json from {path}: {err}"
                    ))
                })
            })
            .transpose()
    }
}

/// Parse `<command> --syntax --query-text --top-k [--cursor-json PATH|-]`
/// with the pinned-generation flags: the shape of the lexical, symbol,
/// runtime-metadata, history and structural routes.
fn parse_keyset_page_query(
    common: &mut CommonOptions,
    rest: &mut VecDeque<String>,
    command: &'static str,
) -> CliResult<KeysetPageQueryArgs> {
    parse_keyset_page_query_with(common, rest, command, |_current, _rest| Ok(false))
}

/// [`parse_keyset_page_query`] with one more local-flag hook, for the
/// commands that take flags beside the page query (`history --order`).
fn parse_keyset_page_query_with(
    common: &mut CommonOptions,
    rest: &mut VecDeque<String>,
    command: &'static str,
    mut parse_extra: impl FnMut(&str, &mut VecDeque<String>) -> CliResult<bool>,
) -> CliResult<KeysetPageQueryArgs> {
    let mut generation_args = PinnedGenerationArgs::default();
    let mut syntax: Option<TextQuerySyntax> = None;
    let mut query_text: Option<String> = None;
    let mut top_k: Option<u32> = None;
    let mut cursor_json: Option<String> = None;
    parse_query_command_flags(
        common,
        &mut generation_args,
        rest,
        command,
        |current, rest| match current {
            "--syntax" => {
                syntax = Some(parse_syntax(&take_value(rest, "--syntax")?)?);
                Ok(true)
            }
            "--query-text" => {
                query_text = Some(take_value(rest, "--query-text")?);
                Ok(true)
            }
            "--top-k" => {
                top_k = Some(parse_u32_flag(rest, "--top-k")?);
                Ok(true)
            }
            "--cursor-json" => {
                cursor_json = Some(take_value(rest, "--cursor-json")?);
                Ok(true)
            }
            other => parse_extra(other, rest),
        },
    )?;
    let generation = generation_args.into_generation_pin()?;
    let syntax = syntax.ok_or_else(|| CliError::usage("missing --syntax".to_string()))?;
    let query_text =
        query_text.ok_or_else(|| CliError::usage("missing --query-text".to_string()))?;
    let top_k = top_k.ok_or_else(|| CliError::usage("missing --top-k".to_string()))?;
    Ok(KeysetPageQueryArgs {
        text_query: TextQueryRequest {
            syntax,
            query_text,
            constraints: QueryConstraintSetV1::unconstrained(),
            generation: Some(generation),
            generation_selector: None,
            top_k,
            cursor: None,
        },
        cursor_json: cursor_json.map(|path| (path, command)),
    })
}

/// The history order a `--order` value names.
fn parse_history_order(value: &str) -> CliResult<HistoryOrderV1> {
    HistoryOrderV1::from_code_str(value).ok_or_else(|| {
        CliError::usage(format!(
            "invalid --order `{value}`; expected `recency` or `relevance`"
        ))
    })
}

fn parse_history(common: &mut CommonOptions, rest: &mut VecDeque<String>) -> CliResult<CliRequest> {
    let mut order: Option<HistoryOrderV1> = None;
    let page =
        parse_keyset_page_query_with(common, rest, "history", |current, rest| match current {
            "--order" => {
                order = Some(parse_history_order(&take_value(rest, "--order")?)?);
                Ok(true)
            }
            _ => Ok(false),
        })?;
    let order = order.ok_or_else(|| CliError::usage("missing --order".to_string()))?;
    let cursor = page.cursor()?;
    Ok(CliRequest::History(HistoryQueryRequest {
        text_query: page.text_query,
        order,
        cursor,
    }))
}

fn parse_structural(
    common: &mut CommonOptions,
    rest: &mut VecDeque<String>,
) -> CliResult<CliRequest> {
    let page = parse_keyset_page_query(common, rest, "structural")?;
    let cursor = page.cursor()?;
    Ok(CliRequest::Structural(StructuralQueryRequest {
        text_query: page.text_query,
        cursor,
    }))
}

/// J7Q-05: parse the `--repo-id <ID> --revision-id <REV>` pair shared by the
/// control-plane commands (`readiness`, `doctor`).
///
/// These commands query the activation catalog, not a sealed generation, so
/// they carry no generation pin, syntax, or `top-k`. Missing `--repo-id` /
/// `--revision-id` are rejected fail-closed (`EXIT_USAGE`), never defaulted.
/// `command` names the subcommand only so the unknown-flag error is precise.
fn parse_repo_revision(
    common: &mut CommonOptions,
    rest: &mut VecDeque<String>,
    command: &str,
) -> CliResult<(RepoId, RevisionId)> {
    let mut repo_id: Option<String> = None;
    let mut revision_id: Option<String> = None;
    while let Some(current) = rest.pop_front() {
        if common.parse_flag(&current, rest)? {
            continue;
        }
        match current.as_str() {
            "--repo-id" => {
                repo_id = Some(take_value(rest, "--repo-id")?);
            }
            "--revision-id" => {
                revision_id = Some(take_value(rest, "--revision-id")?);
            }
            other => {
                return Err(CliError::usage(format!("unknown {command} flag `{other}`")));
            }
        }
    }
    Ok((
        RepoId::new(repo_id.ok_or_else(|| CliError::usage("missing --repo-id".to_string()))?)
            .map_err(|error| CliError::usage(format!("invalid --repo-id: {error}")))?,
        RevisionId::new(
            revision_id.ok_or_else(|| CliError::usage("missing --revision-id".to_string()))?,
        )
        .map_err(|error| CliError::usage(format!("invalid --revision-id: {error}")))?,
    ))
}

/// J7Q-05: parse `readiness --repo-id <ID> --revision-id <REV>`.
fn parse_readiness(
    common: &mut CommonOptions,
    rest: &mut VecDeque<String>,
) -> CliResult<CliRequest> {
    let (repo_id, revision_id) = parse_repo_revision(common, rest, "readiness")?;
    Ok(CliRequest::Readiness {
        repo_id,
        revision_id,
    })
}

/// QI-BB-015: parse `metrics`, which takes only the global flags.
fn parse_metrics(common: &mut CommonOptions, rest: &mut VecDeque<String>) -> CliResult<CliRequest> {
    while let Some(current) = rest.pop_front() {
        if common.parse_flag(&current, rest)? {
            continue;
        }
        return Err(CliError::usage(format!("unknown metrics flag `{current}`")));
    }
    Ok(CliRequest::Metrics)
}

/// QI-BB-026: parse `quarantine list` and `quarantine discard …`.
///
/// A discard names one entry the way `quarantine list` printed it: a
/// generation directory by track, path and reason (the detail is optional
/// and echoed back), or a `RepoMap` file by name. Every value is required
/// so the daemon can refuse a stale listing typed; nothing is defaulted.
fn parse_quarantine(
    common: &mut CommonOptions,
    rest: &mut VecDeque<String>,
) -> CliResult<CliRequest> {
    let verb = loop {
        let Some(current) = rest.pop_front() else {
            return Err(CliError::usage(
                "quarantine requires `list` or `discard`".to_string(),
            ));
        };
        if common.parse_flag(&current, rest)? {
            continue;
        }
        break current;
    };
    match verb.as_str() {
        "list" => {
            while let Some(current) = rest.pop_front() {
                if common.parse_flag(&current, rest)? {
                    continue;
                }
                return Err(CliError::usage(format!(
                    "unknown quarantine list flag `{current}`"
                )));
            }
            Ok(CliRequest::QuarantineList)
        }
        "discard" => parse_quarantine_discard(common, rest),
        other => Err(CliError::usage(format!(
            "unknown quarantine verb `{other}`; expected `list` or `discard`"
        ))),
    }
}

fn parse_quarantine_discard(
    common: &mut CommonOptions,
    rest: &mut VecDeque<String>,
) -> CliResult<CliRequest> {
    let mut track: Option<String> = None;
    let mut path: Option<String> = None;
    let mut reason: Option<String> = None;
    let mut detail: Option<String> = None;
    let mut repomap_file: Option<String> = None;
    while let Some(current) = rest.pop_front() {
        if common.parse_flag(&current, rest)? {
            continue;
        }
        match current.as_str() {
            "--track" => track = Some(take_value(rest, "--track")?),
            "--path" => path = Some(take_value(rest, "--path")?),
            "--reason" => reason = Some(take_value(rest, "--reason")?),
            "--detail" => detail = Some(take_value(rest, "--detail")?),
            "--repomap-file" => repomap_file = Some(take_value(rest, "--repomap-file")?),
            other => {
                return Err(CliError::usage(format!(
                    "unknown quarantine discard flag `{other}`"
                )));
            }
        }
    }
    let target = match (repomap_file, track, path) {
        (Some(file_name), None, None) => {
            QuarantineTargetV1::RepoMapFile(QuarantinedRepoMapFileEntryV1 {
                file_name,
                reason: reason.ok_or_else(|| {
                    CliError::usage("--repomap-file requires --reason as listed".to_string())
                })?,
            })
        }
        (None, Some(track), Some(path)) => {
            let track = match track.as_str() {
                "lexical" => SearchPlaneTrackKind::Lexical,
                "semantic" => SearchPlaneTrackKind::Semantic,
                other => {
                    return Err(CliError::usage(format!(
                        "unsupported quarantine track `{other}`; expected `lexical` or `semantic`"
                    )));
                }
            };
            QuarantineTargetV1::Generation(QuarantinedGenerationEntryV1 {
                track,
                path,
                reason: reason.ok_or_else(|| {
                    CliError::usage("--path requires --reason as listed".to_string())
                })?,
                detail: detail.unwrap_or_default(),
            })
        }
        (None, _, _) => {
            return Err(CliError::usage(
                "quarantine discard needs --track and --path (a generation directory) or --repomap-file"
                    .to_string(),
            ));
        }
        (Some(_), _, _) => {
            return Err(CliError::usage(
                "--repomap-file cannot be combined with --track/--path".to_string(),
            ));
        }
    };
    Ok(CliRequest::QuarantineDiscard(target))
}

/// J7Q-05: parse `doctor --repo-id <ID> --revision-id <REV>`.
///
/// `doctor` is the composite read-only diagnosis: it fuses the activation
/// catalog listing with per-track serve-time resolution into one operator
/// answer. It takes the same `(repo, revision)` pair as `readiness` and no more.
fn parse_doctor(common: &mut CommonOptions, rest: &mut VecDeque<String>) -> CliResult<CliRequest> {
    let (repo_id, revision_id) = parse_repo_revision(common, rest, "doctor")?;
    Ok(CliRequest::Doctor {
        repo_id,
        revision_id,
    })
}

fn parse_semantic(
    common: &mut CommonOptions,
    rest: &mut VecDeque<String>,
) -> CliResult<CliRequest> {
    let mut generation_args = PinnedGenerationArgs::default();
    let mut query_text: Option<String> = None;
    let mut top_k: Option<u32> = None;
    let mut scope_query_text: Option<String> = None;
    let mut scope_syntax: Option<TextQuerySyntax> = None;
    let mut scope_top_k: Option<u32> = None;
    parse_query_command_flags(
        common,
        &mut generation_args,
        rest,
        "semantic",
        |current, rest| match current {
            "--query-text" => {
                query_text = Some(take_value(rest, "--query-text")?);
                Ok(true)
            }
            "--top-k" => {
                top_k = Some(parse_u32_flag(rest, "--top-k")?);
                Ok(true)
            }
            "--scope-query" => {
                scope_query_text = Some(take_value(rest, "--scope-query")?);
                Ok(true)
            }
            "--scope-syntax" => {
                scope_syntax = Some(parse_syntax(&take_value(rest, "--scope-syntax")?)?);
                Ok(true)
            }
            "--scope-top-k" => {
                scope_top_k = Some(parse_u32_flag(rest, "--scope-top-k")?);
                Ok(true)
            }
            _ => Ok(false),
        },
    )?;
    let generation = generation_args.into_generation_pin()?;
    let lexical_scope = match (scope_query_text, scope_syntax) {
        (None, None) => {
            if scope_top_k.is_some() {
                return Err(CliError::usage(
                    "--scope-top-k is only valid when --scope-query is set".to_string(),
                ));
            }
            None
        }
        (Some(query), Some(syntax)) => {
            let scope_top_k = scope_top_k.ok_or_else(|| {
                CliError::usage(
                    "missing --scope-top-k (required when --scope-query is set)".to_string(),
                )
            })?;
            Some(TextQueryRequest {
                syntax,
                query_text: query,
                constraints: quanta_index_contract::QueryConstraintSetV1::unconstrained(),
                generation: Some(generation.clone()),
                generation_selector: None,
                top_k: scope_top_k,
                cursor: None,
            })
        }
        (Some(_), None) => {
            return Err(CliError::usage(
                "semantic scope requires --scope-syntax".to_string(),
            ));
        }
        (None, Some(_)) => {
            return Err(CliError::usage(
                "semantic scope requires --scope-query".to_string(),
            ));
        }
    };
    Ok(CliRequest::Semantic(SemanticQueryRequest {
        query_text: query_text
            .ok_or_else(|| CliError::usage("missing --query-text".to_string()))?,
        constraints: QueryConstraintSetV1::unconstrained(),
        generation: Some(generation),
        generation_selector: None,
        lexical_scope,
        top_k: top_k.ok_or_else(|| CliError::usage("missing --top-k".to_string()))?,
    }))
}

/// `hybrid` (QI-BB-018): the lexical lane's query and syntax, the dense
/// lane's text, and the fused `top_k`.
///
/// The two lanes run independently over the pinned generation and their
/// union is fused by RRF; a document with no lexical overlap can enter the
/// page on dense relevance alone.
fn parse_hybrid(common: &mut CommonOptions, rest: &mut VecDeque<String>) -> CliResult<CliRequest> {
    let mut generation_args = PinnedGenerationArgs::default();
    let mut query_text: Option<String> = None;
    let mut syntax: Option<TextQuerySyntax> = None;
    let mut semantic_query_text: Option<String> = None;
    let mut top_k: Option<u32> = None;
    parse_query_command_flags(
        common,
        &mut generation_args,
        rest,
        "hybrid",
        |current, rest| match current {
            "--query-text" => {
                query_text = Some(take_value(rest, "--query-text")?);
                Ok(true)
            }
            "--syntax" => {
                syntax = Some(parse_syntax(&take_value(rest, "--syntax")?)?);
                Ok(true)
            }
            "--semantic-query-text" => {
                semantic_query_text = Some(take_value(rest, "--semantic-query-text")?);
                Ok(true)
            }
            "--top-k" => {
                top_k = Some(parse_u32_flag(rest, "--top-k")?);
                Ok(true)
            }
            _ => Ok(false),
        },
    )?;
    let generation = generation_args.into_generation_pin()?;
    let top_k = top_k.ok_or_else(|| CliError::usage("missing --top-k".to_string()))?;
    let text_query = TextQueryRequest {
        syntax: syntax.ok_or_else(|| CliError::usage("missing --syntax".to_string()))?,
        query_text: query_text
            .ok_or_else(|| CliError::usage("missing --query-text".to_string()))?,
        constraints: QueryConstraintSetV1::unconstrained(),
        generation: Some(generation.clone()),
        generation_selector: None,
        top_k,
        cursor: None,
    };
    Ok(CliRequest::Hybrid(HybridQueryRequest {
        text_query,
        semantic_query_text: semantic_query_text
            .ok_or_else(|| CliError::usage("missing --semantic-query-text".to_string()))?,
        generation: Some(generation),
        generation_selector: None,
        top_k,
    }))
}

fn parse_hybrid_seed(
    common: &mut CommonOptions,
    rest: &mut VecDeque<String>,
) -> CliResult<CliRequest> {
    let mut generation_args = PinnedGenerationArgs::default();
    let mut lexical_query_text: Option<String> = None;
    let mut lexical_syntax: Option<TextQuerySyntax> = None;
    let mut semantic_query_text: Option<String> = None;
    let mut top_k: Option<u32> = None;
    parse_query_command_flags(
        common,
        &mut generation_args,
        rest,
        "hybrid-seed",
        |current, rest| match current {
            "--lexical-query" => {
                lexical_query_text = Some(take_value(rest, "--lexical-query")?);
                Ok(true)
            }
            "--lexical-syntax" => {
                lexical_syntax = Some(parse_syntax(&take_value(rest, "--lexical-syntax")?)?);
                Ok(true)
            }
            "--semantic-query" => {
                semantic_query_text = Some(take_value(rest, "--semantic-query")?);
                Ok(true)
            }
            "--top-k" => {
                top_k = Some(parse_u32_flag(rest, "--top-k")?);
                Ok(true)
            }
            _ => Ok(false),
        },
    )?;
    let generation = generation_args.into_generation_pin()?;
    let text_query_top_k = top_k.ok_or_else(|| CliError::usage("missing --top-k".to_string()))?;
    let text_query = TextQueryRequest {
        syntax: lexical_syntax
            .ok_or_else(|| CliError::usage("missing --lexical-syntax".to_string()))?,
        query_text: lexical_query_text
            .ok_or_else(|| CliError::usage("missing --lexical-query".to_string()))?,
        constraints: QueryConstraintSetV1::unconstrained(),
        generation: Some(generation.clone()),
        generation_selector: None,
        top_k: text_query_top_k,
        cursor: None,
    };
    Ok(CliRequest::HybridSeed(HybridSeedQueryRequest {
        text_query,
        semantic_query_text: semantic_query_text
            .ok_or_else(|| CliError::usage("missing --semantic-query".to_string()))?,
        generation: Some(generation),
        generation_selector: None,
        dense_corpora: Vec::new(),
        top_k: top_k.ok_or_else(|| CliError::usage("missing --top-k".to_string()))?,
    }))
}

fn parse_explain(common: &mut CommonOptions, rest: &mut VecDeque<String>) -> CliResult<CliRequest> {
    let mut generation_args = PinnedGenerationArgs::default();
    let mut candidate_json: Option<String> = None;
    let mut hybrid_candidate_json: Option<String> = None;
    let mut syntax: Option<TextQuerySyntax> = None;
    let mut query_text: Option<String> = None;
    let mut semantic_query_text: Option<String> = None;
    let mut top_k: Option<u32> = None;
    parse_query_command_flags(
        common,
        &mut generation_args,
        rest,
        "explain",
        |current, rest| match current {
            "--candidate-json" => {
                candidate_json = Some(take_value(rest, "--candidate-json")?);
                Ok(true)
            }
            "--hybrid-candidate-json" => {
                hybrid_candidate_json = Some(take_value(rest, "--hybrid-candidate-json")?);
                Ok(true)
            }
            "--syntax" => {
                syntax = Some(parse_syntax(&take_value(rest, "--syntax")?)?);
                Ok(true)
            }
            "--query-text" => {
                query_text = Some(take_value(rest, "--query-text")?);
                Ok(true)
            }
            "--semantic-query-text" => {
                semantic_query_text = Some(take_value(rest, "--semantic-query-text")?);
                Ok(true)
            }
            "--top-k" => {
                top_k = Some(parse_u32_flag(rest, "--top-k")?);
                Ok(true)
            }
            _ => Ok(false),
        },
    )?;
    let generation = generation_args.into_generation_pin()?;
    // The candidate is the row as the route that ranked it carried it: a
    // lexical/semantic page row, or a hybrid row with its lane provenance.
    let candidate = match (candidate_json, hybrid_candidate_json) {
        (Some(path), None) => ExplainCandidateV1::Lexical(read_candidate_json(&path)?),
        (None, Some(path)) => ExplainCandidateV1::Hybrid(read_hybrid_candidate_json(&path)?),
        (None, None) => {
            return Err(CliError::usage(
                "missing --candidate-json or --hybrid-candidate-json".to_string(),
            ));
        }
        (Some(_), Some(_)) => {
            return Err(CliError::usage(
                "--candidate-json and --hybrid-candidate-json are mutually exclusive".to_string(),
            ));
        }
    };
    // `--syntax` and `--query-text` name the query the candidate came from;
    // both or neither, since a query without its syntax cannot be lowered.
    // A lexical explain does not page, so its `top_k` is the smallest
    // accepted value; a hybrid explain re-runs the lanes at the fused
    // `top_k` the hybrid ran with, which the caller must name.
    let is_hybrid = matches!(candidate, ExplainCandidateV1::Hybrid(_));
    let text_query_top_k = match (is_hybrid, top_k) {
        (true, Some(top_k)) => top_k,
        (true, None) => {
            return Err(CliError::usage(
                "--hybrid-candidate-json re-runs both lanes at the hybrid's fused top_k; pass --top-k"
                    .to_string(),
            ));
        }
        (false, None) => 1,
        (false, Some(_)) => {
            return Err(CliError::usage(
                "--top-k applies to --hybrid-candidate-json only; a lexical explain does not page"
                    .to_string(),
            ));
        }
    };
    let text_query = match (syntax, query_text) {
        (Some(syntax), Some(query_text)) => Some(TextQueryRequest {
            syntax,
            query_text,
            constraints: QueryConstraintSetV1::unconstrained(),
            generation: Some(generation.clone()),
            generation_selector: None,
            top_k: text_query_top_k,
            cursor: None,
        }),
        (None, None) => None,
        (Some(_), None) => {
            return Err(CliError::usage(
                "--syntax requires --query-text".to_string(),
            ));
        }
        (None, Some(_)) => {
            return Err(CliError::usage(
                "--query-text requires --syntax".to_string(),
            ));
        }
    };
    match (is_hybrid, &text_query, &semantic_query_text) {
        (true, None, _) => {
            return Err(CliError::usage(
                "--hybrid-candidate-json explains under the query it was fused for; pass --syntax and --query-text"
                    .to_string(),
            ));
        }
        (true, Some(_), None) => {
            return Err(CliError::usage(
                "--hybrid-candidate-json re-derives the dense lane under the query it was fused for; pass --semantic-query-text"
                    .to_string(),
            ));
        }
        (false, _, Some(_)) => {
            return Err(CliError::usage(
                "--semantic-query-text applies to --hybrid-candidate-json only; a lexical candidate has no dense lane"
                    .to_string(),
            ));
        }
        (true, Some(_), Some(_)) | (false, _, None) => {}
    }
    Ok(CliRequest::Explain {
        generation,
        candidate,
        text_query,
        semantic_query_text,
    })
}

fn parse_repomap(common: &mut CommonOptions, rest: &mut VecDeque<String>) -> CliResult<CliRequest> {
    let mut generation_args = PinnedGenerationArgs::default();
    let mut query_text: Option<String> = None;
    let mut top_k: Option<u32> = None;
    let mut token_budget: Option<u32> = None;
    let mut focus_subjects: Vec<RepoMapFocusSubjectDto> = Vec::new();
    parse_query_command_flags(
        common,
        &mut generation_args,
        rest,
        "repomap",
        |current, rest| match current {
            "--query-text" => {
                query_text = Some(take_value(rest, "--query-text")?);
                Ok(true)
            }
            "--top-k" => {
                top_k = Some(parse_u32_flag(rest, "--top-k")?);
                Ok(true)
            }
            "--token-budget" => {
                token_budget = Some(parse_u32_flag(rest, "--token-budget")?);
                Ok(true)
            }
            "--focus-subject" => {
                focus_subjects.push(parse_focus_subject(&take_value(rest, "--focus-subject")?)?);
                Ok(true)
            }
            _ => Ok(false),
        },
    )?;
    let generation = generation_args.into_generation_pin()?;
    Ok(CliRequest::RepoMap(RepoMapQueryRequest {
        repo_id: generation.repo_id,
        revision_id: generation.revision_id,
        manifest_generation: generation.manifest_generation,
        query_text: query_text
            .ok_or_else(|| CliError::usage("missing --query-text".to_string()))?,
        top_k: top_k.ok_or_else(|| CliError::usage("missing --top-k".to_string()))?,
        token_budget: token_budget
            .ok_or_else(|| CliError::usage("missing --token-budget".to_string()))?,
        focus_subjects,
    }))
}

fn parse_generation_pin(
    repo_id: Option<String>,
    revision_id: Option<String>,
    manifest_generation: Option<u64>,
) -> CliResult<GenerationPin> {
    Ok(GenerationPin::new(
        RepoId::new(repo_id.ok_or_else(|| CliError::usage("missing --repo-id".to_string()))?)
            .map_err(|error| CliError::usage(format!("invalid --repo-id: {error}")))?,
        RevisionId::new(
            revision_id.ok_or_else(|| CliError::usage("missing --revision-id".to_string()))?,
        )
        .map_err(|error| CliError::usage(format!("invalid --revision-id: {error}")))?,
        ManifestGeneration::new(
            manifest_generation
                .ok_or_else(|| CliError::usage("missing --manifest-generation".to_string()))?,
        ),
    ))
}

fn take_value(rest: &mut VecDeque<String>, flag: &str) -> CliResult<String> {
    rest.pop_front()
        .ok_or_else(|| CliError::usage(format!("{flag} requires a value")))
}

fn parse_u32_flag(rest: &mut VecDeque<String>, flag: &str) -> CliResult<u32> {
    let value = take_value(rest, flag)?;
    value.parse::<u32>().map_err(|err| {
        CliError::usage(format!(
            "{flag} requires an unsigned integer, got `{value}`: {err}"
        ))
    })
}

fn dispatch_query_request(
    client: &QuantaIndex,
    request: CliRequest,
) -> CliResult<SearchPlaneQueryIpcResponseEnvelope> {
    let payload = match request {
        CliRequest::Lexical(text) => SearchPlaneQueryIpcResponse::Text(
            client
                .lexical()
                .query_request(text)
                .map_err(map_sdk_error)?,
        ),
        CliRequest::Symbol(symbol) => SearchPlaneQueryIpcResponse::Symbol(
            client
                .symbol()
                .query_request(symbol)
                .map_err(map_sdk_error)?,
        ),
        CliRequest::Semantic(semantic) => SearchPlaneQueryIpcResponse::Semantic(
            client
                .semantic()
                .query_request(semantic)
                .map_err(map_sdk_error)?,
        ),
        CliRequest::Hybrid(hybrid) => SearchPlaneQueryIpcResponse::Hybrid(
            client
                .search()
                .hybrid_request(hybrid)
                .map_err(map_sdk_error)?,
        ),
        CliRequest::HybridSeed(hybrid) => SearchPlaneQueryIpcResponse::HybridSeed(
            client
                .search()
                .hybrid_seed_request(hybrid)
                .map_err(map_sdk_error)?,
        ),
        CliRequest::RepoMap(repomap) => SearchPlaneQueryIpcResponse::RepoMapQuery(
            client.repomap().query(repomap).map_err(map_sdk_error)?,
        ),
        CliRequest::RuntimeMetadata(runtime) => SearchPlaneQueryIpcResponse::RuntimeMetadata(
            client
                .runtime()
                .query_request(runtime)
                .map_err(map_sdk_error)?,
        ),
        CliRequest::Explain {
            generation,
            candidate,
            text_query,
            semantic_query_text,
        } => SearchPlaneQueryIpcResponse::Explain(
            match (candidate, text_query, semantic_query_text) {
                (ExplainCandidateV1::Lexical(candidate), Some(text_query), None) => client
                    .search()
                    .explain_under_query(generation, candidate, text_query),
                (ExplainCandidateV1::Lexical(candidate), None, None) => {
                    client.search().explain(generation, candidate)
                }
                (ExplainCandidateV1::Hybrid(row), Some(text_query), Some(semantic_query_text)) => {
                    client.search().explain_hybrid_under_queries(
                        generation,
                        row,
                        text_query,
                        semantic_query_text,
                    )
                }
                (ExplainCandidateV1::Hybrid(_), _, _) => {
                    return Err(CliError::usage(
                        "a hybrid candidate explains under both its queries; pass --syntax, --query-text and --semantic-query-text"
                            .to_string(),
                    ));
                }
                (ExplainCandidateV1::Lexical(_), _, Some(_)) => {
                    return Err(CliError::usage(
                        "a lexical candidate has no dense lane; drop --semantic-query-text"
                            .to_string(),
                    ));
                }
            }
            .map_err(map_sdk_error)?,
        ),
        CliRequest::History(history) => SearchPlaneQueryIpcResponse::History(
            client
                .history()
                .query_request(history)
                .map_err(map_sdk_error)?,
        ),
        CliRequest::Structural(structural) => SearchPlaneQueryIpcResponse::Structural(
            client
                .structural()
                .query_request(structural)
                .map_err(map_sdk_error)?,
        ),
        // `readiness` / `doctor` are control-plane commands; `run_inner` branches
        // them before reaching the query dispatcher. Reaching here is a routing
        // bug, so fail-closed with a typed protocol error rather than fabricate a
        // query.
        CliRequest::Readiness { .. }
        | CliRequest::Doctor { .. }
        | CliRequest::Metrics
        | CliRequest::QuarantineList
        | CliRequest::QuarantineDiscard(_) => {
            return Err(CliError::protocol(
                "readiness/doctor/metrics/quarantine are control-plane commands and must not reach the query dispatcher"
                    .to_string(),
            ));
        }
    };
    Ok(SearchPlaneQueryIpcResponseEnvelope {
        request_id: REQUEST_ID,
        payload,
    })
}

fn map_sdk_error(error: SdkError) -> CliError {
    match error {
        SdkError::Usage(message) => CliError::usage(message),
        SdkError::Protocol(message) => CliError::protocol(message),
        SdkError::Serialization(message) => {
            CliError::protocol(format!("ipc serialization failed: {message}"))
        }
        SdkError::Transport(error) => CliError::transport(format!("ipc request failed: {error}")),
        SdkError::Remote {
            code,
            message,
            repair,
        } => match render_remote_error_text(&code, &message, repair.as_ref()) {
            Ok(text) => CliError::remote(text),
            Err(err) => err,
        },
        SdkError::Binding {
            route,
            axis,
            expected,
            actual,
        } => CliError::protocol(format!(
            "response binding failed on route `{route}` axis `{axis}`: expected {expected}, got {actual}"
        )),
        SdkError::PlaneUnavailable { plane } => CliError::usage(format!(
            "{plane} transport is not configured for this client profile"
        )),
    }
}

fn parse_u64_flag(rest: &mut VecDeque<String>, flag: &str) -> CliResult<u64> {
    let value = take_value(rest, flag)?;
    value.parse::<u64>().map_err(|err| {
        CliError::usage(format!(
            "{flag} requires an unsigned integer, got `{value}`: {err}"
        ))
    })
}

fn parse_syntax(value: &str) -> CliResult<TextQuerySyntax> {
    TextQuerySyntax::from_str_value(value).ok_or_else(|| {
        CliError::usage(format!(
            "unsupported syntax `{value}`; expected `native` or `sourcegraph`"
        ))
    })
}

fn parse_repomap_doc_type(value: &str) -> CliResult<RepoMapDocType> {
    match value {
        "File" | "file" => Ok(RepoMapDocType::File),
        "Module" | "module" => Ok(RepoMapDocType::Module),
        "Symbol" | "symbol" => Ok(RepoMapDocType::Symbol),
        "Chunk" | "chunk" => Ok(RepoMapDocType::Chunk),
        other => Err(CliError::usage(format!(
            "unsupported repo-map doc type `{other}`; expected file|module|symbol|chunk"
        ))),
    }
}

fn parse_focus_subject(value: &str) -> CliResult<RepoMapFocusSubjectDto> {
    let (subject_identity, subject_doc_type) = value.split_once(':').ok_or_else(|| {
        CliError::usage(format!(
            "--focus-subject expects <subject_identity>:<subject_doc_type>, got `{value}`"
        ))
    })?;
    if subject_identity.is_empty() || subject_doc_type.is_empty() {
        return Err(CliError::usage(format!(
            "--focus-subject requires non-empty identity and doc type, got `{value}`"
        )));
    }
    Ok(RepoMapFocusSubjectDto {
        subject_identity: subject_identity.to_string(),
        subject_doc_type: parse_repomap_doc_type(subject_doc_type)?,
    })
}

fn read_candidate_json(candidate_path: &str) -> CliResult<LexicalCandidate> {
    let raw = read_json_text(candidate_path, "candidate")?;
    serde_json::from_str::<LexicalCandidate>(&raw).map_err(|err| {
        CliError::usage(format!(
            "failed to decode candidate json from {candidate_path}: {err}"
        ))
    })
}

fn read_hybrid_candidate_json(candidate_path: &str) -> CliResult<HybridCandidateV1> {
    let raw = read_json_text(candidate_path, "hybrid candidate")?;
    serde_json::from_str::<HybridCandidateV1>(&raw).map_err(|err| {
        CliError::usage(format!(
            "failed to decode hybrid candidate json from {candidate_path}: {err}"
        ))
    })
}

/// Read one JSON document's text from a path, or from stdin for `-`;
/// `what` names the document in errors.
fn read_json_text(path: &str, what: &str) -> CliResult<String> {
    if path == "-" {
        let mut input = String::new();
        let _bytes_read = std::io::stdin().read_to_string(&mut input).map_err(|err| {
            CliError::transport(format!("failed reading {what} json from stdin: {err}"))
        })?;
        return Ok(input);
    }
    fs::read_to_string(path).map_err(|err| {
        CliError::transport(format!("failed reading {what} json from {path}: {err}"))
    })
}

fn validate_response_kind(
    expected: CommandKind,
    response: &SearchPlaneQueryIpcResponseEnvelope,
) -> CliResult<()> {
    match (&expected, &response.payload) {
        (_, SearchPlaneQueryIpcResponse::Error(error)) => Err(CliError::remote(format!(
            "{}: {}",
            error.code, error.message
        ))),
        (CommandKind::Lexical, SearchPlaneQueryIpcResponse::Text(_))
        | (CommandKind::Symbol, SearchPlaneQueryIpcResponse::Symbol(_))
        | (CommandKind::Semantic, SearchPlaneQueryIpcResponse::Semantic(_))
        | (CommandKind::Hybrid, SearchPlaneQueryIpcResponse::Hybrid(_))
        | (CommandKind::HybridSeed, SearchPlaneQueryIpcResponse::HybridSeed(_))
        | (CommandKind::Explain, SearchPlaneQueryIpcResponse::Explain(_))
        | (CommandKind::RepoMap, SearchPlaneQueryIpcResponse::RepoMapQuery(_))
        | (CommandKind::RuntimeMetadata, SearchPlaneQueryIpcResponse::RuntimeMetadata(_))
        | (CommandKind::History, SearchPlaneQueryIpcResponse::History(_))
        | (CommandKind::Structural, SearchPlaneQueryIpcResponse::Structural(_)) => Ok(()),
        _ => Err(CliError::protocol(format!(
            "response kind `{}` does not match requested command `{}`",
            response_kind_name(&response.payload),
            command_kind_name(expected)
        ))),
    }
}

fn render_response(
    output: OutputMode,
    response: &SearchPlaneQueryIpcResponseEnvelope,
    stdout: &mut dyn Write,
) -> CliResult<()> {
    match output {
        OutputMode::Json => {
            serde_json::to_writer_pretty(&mut *stdout, response).map_err(|err| {
                CliError::protocol(format!("failed to encode json output: {err}"))
            })?;
            stdout
                .write_all(b"\n")
                .map_err(|err| CliError::transport(format!("failed writing stdout: {err}")))?;
            Ok(())
        }
        OutputMode::Pretty => {
            let mut rendered = String::new();
            render_pretty(response, &mut rendered)?;
            stdout
                .write_all(rendered.as_bytes())
                .map_err(|err| CliError::transport(format!("failed writing stdout: {err}")))?;
            Ok(())
        }
        OutputMode::Prometheus => Err(prometheus_is_metrics_only("a query command")),
    }
}

/// J7Q-05: render a control-plane [`GenerationStatusReport`] to a string.
///
/// `json` mode serializes the report directly (the DTO carries a manual
/// `Serialize`), so the machine surface is the wire shape verbatim. `pretty`
/// mode emits a stable, line-oriented form: a header line plus one line per
/// activated track in `tracks` declaration order. An empty `tracks` vec renders
/// an explicit `tracks: 0 (none activated)` line — never a silent blank — to
/// keep "nothing activated" distinct from a malformed report.
fn render_readiness(report: &GenerationStatusReport, output: OutputMode) -> CliResult<String> {
    match output {
        OutputMode::Json => serde_json::to_string_pretty(report)
            .map(|mut text| {
                text.push('\n');
                text
            })
            .map_err(|err| CliError::protocol(format!("failed to encode json output: {err}"))),
        OutputMode::Prometheus => Err(prometheus_is_metrics_only("readiness")),
        OutputMode::Pretty => {
            let mut rendered = String::new();
            fmt_ok(writeln!(rendered, "kind: readiness"))?;
            fmt_ok(writeln!(
                rendered,
                "generation_status: repo_id={} revision_id={}",
                report.repo_id.as_str(),
                report.revision_id.as_str()
            ))?;
            if report.tracks.is_empty() {
                fmt_ok(writeln!(rendered, "tracks: 0 (none activated)"))?;
                return Ok(rendered);
            }
            fmt_ok(writeln!(rendered, "tracks: {}", report.tracks.len()))?;
            for (index, record) in report.tracks.iter().enumerate() {
                let display_index = index.checked_add(1).ok_or_else(|| {
                    CliError::protocol("readiness track index overflow".to_string())
                })?;
                fmt_ok(writeln!(
                    rendered,
                    "{}. track={} manifest_generation={} manifest_digest={}",
                    display_index,
                    record.track.as_code_str(),
                    record.manifest_generation.get(),
                    record.manifest_digest
                ))?;
            }
            // The semantic content roots the active pair was activated
            // under (QI-BB-028): what the plane sealed, not what the
            // producer asked for.
            if let Some(roots) = &report.semantic_content {
                fmt_ok(writeln!(
                    rendered,
                    "semantic_content: row_root={} membership_root={}",
                    roots.row_root_digest, roots.membership_root_digest
                ))?;
            }
            Ok(rendered)
        }
    }
}

/// QI-BB-015: render a [`MetricsSnapshotV1`].
///
/// `json` is the wire shape verbatim. `pretty` is line-oriented: one line
/// per counter and gauge, a header plus one bucket line per histogram, and
/// the diagnostic tallies last. `prometheus` is the text exposition format:
/// a `# TYPE` line per metric, `_bucket{le="…"}` / `_sum` / `_count` series
/// per histogram. The wire carries only finite bounds, so both renderers
/// spell the `+Inf` bucket from the histogram's `count`.
fn render_metrics(snapshot: &MetricsSnapshotV1, output: OutputMode) -> CliResult<String> {
    match output {
        OutputMode::Json => serde_json::to_string_pretty(snapshot)
            .map(|mut text| {
                text.push('\n');
                text
            })
            .map_err(|err| CliError::protocol(format!("failed to encode json output: {err}"))),
        OutputMode::Pretty => render_metrics_pretty(snapshot),
        OutputMode::Prometheus => render_metrics_prometheus(snapshot),
    }
}

fn render_metrics_pretty(snapshot: &MetricsSnapshotV1) -> CliResult<String> {
    let mut rendered = String::new();
    fmt_ok(writeln!(rendered, "kind: metrics"))?;
    fmt_ok(writeln!(rendered, "counters: {}", snapshot.counters.len()))?;
    for counter in &snapshot.counters {
        fmt_ok(writeln!(rendered, "  {} {}", counter.name, counter.value))?;
    }
    fmt_ok(writeln!(rendered, "gauges: {}", snapshot.gauges.len()))?;
    for gauge in &snapshot.gauges {
        fmt_ok(writeln!(rendered, "  {} {}", gauge.name, gauge.value))?;
    }
    fmt_ok(writeln!(
        rendered,
        "histograms: {}",
        snapshot.histograms.len()
    ))?;
    for histogram in &snapshot.histograms {
        fmt_ok(writeln!(
            rendered,
            "  {} count={} sum={} min={} max={}",
            histogram.name, histogram.count, histogram.sum, histogram.min, histogram.max
        ))?;
        for bucket in &histogram.buckets {
            fmt_ok(writeln!(rendered, "    le={} {}", bucket.le, bucket.count))?;
        }
        fmt_ok(writeln!(rendered, "    le=+Inf {}", histogram.count))?;
    }
    fmt_ok(writeln!(
        rendered,
        "diagnostics: samples_recorded={} samples_dropped={} errors_recorded={} errors_dropped={}",
        snapshot.diagnostics.samples_recorded,
        snapshot.diagnostics.samples_dropped,
        snapshot.diagnostics.errors_recorded,
        snapshot.diagnostics.errors_dropped
    ))?;
    Ok(rendered)
}

fn render_metrics_prometheus(snapshot: &MetricsSnapshotV1) -> CliResult<String> {
    let mut rendered = String::new();
    for counter in &snapshot.counters {
        fmt_ok(writeln!(rendered, "# TYPE {} counter", counter.name))?;
        fmt_ok(writeln!(rendered, "{} {}", counter.name, counter.value))?;
    }
    for gauge in &snapshot.gauges {
        fmt_ok(writeln!(rendered, "# TYPE {} gauge", gauge.name))?;
        fmt_ok(writeln!(rendered, "{} {}", gauge.name, gauge.value))?;
    }
    for histogram in &snapshot.histograms {
        render_prometheus_histogram(&mut rendered, histogram)?;
    }
    for (name, value) in [
        (
            "searchd_obs_samples_recorded_total",
            snapshot.diagnostics.samples_recorded,
        ),
        (
            "searchd_obs_samples_dropped_total",
            snapshot.diagnostics.samples_dropped,
        ),
        (
            "searchd_obs_errors_recorded_total",
            snapshot.diagnostics.errors_recorded,
        ),
        (
            "searchd_obs_errors_dropped_total",
            snapshot.diagnostics.errors_dropped,
        ),
    ] {
        fmt_ok(writeln!(rendered, "# TYPE {name} counter"))?;
        fmt_ok(writeln!(rendered, "{name} {value}"))?;
    }
    Ok(rendered)
}

fn render_prometheus_histogram(
    rendered: &mut String,
    histogram: &MetricHistogramV1,
) -> CliResult<()> {
    fmt_ok(writeln!(rendered, "# TYPE {} histogram", histogram.name))?;
    for bucket in &histogram.buckets {
        fmt_ok(writeln!(
            rendered,
            "{}_bucket{{le=\"{}\"}} {}",
            histogram.name, bucket.le, bucket.count
        ))?;
    }
    fmt_ok(writeln!(
        rendered,
        "{}_bucket{{le=\"+Inf\"}} {}",
        histogram.name, histogram.count
    ))?;
    fmt_ok(writeln!(
        rendered,
        "{}_sum {}",
        histogram.name, histogram.sum
    ))?;
    fmt_ok(writeln!(
        rendered,
        "{}_count {}",
        histogram.name, histogram.count
    ))?;
    Ok(())
}

/// QI-BB-026: render a [`QuarantineInventoryV1`].
///
/// `pretty` prints each entry on one line exactly as `quarantine discard`
/// wants it back, so an operator can copy a line into a discard.
fn render_quarantine_inventory(
    inventory: &QuarantineInventoryV1,
    output: OutputMode,
) -> CliResult<String> {
    match output {
        OutputMode::Json => serde_json::to_string_pretty(inventory)
            .map(|mut text| {
                text.push('\n');
                text
            })
            .map_err(|err| CliError::protocol(format!("failed to encode json output: {err}"))),
        OutputMode::Prometheus => Err(prometheus_is_metrics_only("quarantine")),
        OutputMode::Pretty => {
            let mut rendered = String::new();
            fmt_ok(writeln!(rendered, "kind: quarantine"))?;
            for (label, entries) in [
                ("lexical", &inventory.lexical),
                ("semantic", &inventory.semantic),
            ] {
                fmt_ok(writeln!(rendered, "{label}: {}", entries.len()))?;
                for entry in entries {
                    fmt_ok(writeln!(
                        rendered,
                        "  --track {label} --path {} --reason {} --detail {:?}",
                        entry.path, entry.reason, entry.detail
                    ))?;
                }
            }
            fmt_ok(writeln!(rendered, "repo_map: {}", inventory.repo_map.len()))?;
            for entry in &inventory.repo_map {
                fmt_ok(writeln!(
                    rendered,
                    "  --repomap-file {} --reason {:?}",
                    entry.file_name, entry.reason
                ))?;
            }
            Ok(rendered)
        }
    }
}

/// QI-BB-026: render a [`QuarantineDiscardAck`].
fn render_quarantine_discard(ack: &QuarantineDiscardAck, output: OutputMode) -> CliResult<String> {
    match output {
        OutputMode::Json => serde_json::to_string_pretty(ack)
            .map(|mut text| {
                text.push('\n');
                text
            })
            .map_err(|err| CliError::protocol(format!("failed to encode json output: {err}"))),
        OutputMode::Prometheus => Err(prometheus_is_metrics_only("quarantine")),
        OutputMode::Pretty => {
            let mut rendered = String::new();
            fmt_ok(writeln!(rendered, "kind: quarantine-discard"))?;
            let target = match &ack.target {
                QuarantineTargetV1::Generation(entry) => {
                    format!(
                        "{} {}",
                        entry.track.as_code_str().to_ascii_lowercase(),
                        entry.path
                    )
                }
                QuarantineTargetV1::RepoMapFile(entry) => format!("repo_map {}", entry.file_name),
            };
            let outcome = match ack.outcome {
                QuarantineDiscardOutcomeDtoV1::Discarded { bytes } => {
                    format!("discarded bytes={bytes}")
                }
                QuarantineDiscardOutcomeDtoV1::Absent => "absent".to_string(),
            };
            fmt_ok(writeln!(rendered, "target: {target}"))?;
            fmt_ok(writeln!(rendered, "outcome: {outcome}"))?;
            Ok(rendered)
        }
    }
}

/// J7Q-05: one activated track's slot in the composite [`DoctorReport`].
///
/// `manifest_generation` / `manifest_digest` come from the activation-catalog
/// listing (`generations().status`). `resolver_ok` records whether the
/// serve-time per-track resolver (`generations().current`) corroborated that
/// listing for this track — the cross-check `readiness` does not perform.
#[derive(Clone, Debug)]
struct DoctorTrack {
    track: SearchPlaneTrackKind,
    manifest_generation: u64,
    manifest_digest: String,
    resolver_ok: bool,
    resolver_note: String,
}

/// J7Q-05: composite read-only diagnosis for one `(repo, revision)` pair.
///
/// Fuses the activation-catalog listing with per-track serve-time resolution
/// into one operator answer. `serve_ready` is true only when at least one track
/// is activated AND every activated track resolved consistently. `all_resolvable`
/// isolates the resolver-corroboration verdict from the "is anything activated
/// at all" question, so an empty repo reads `serve_ready=false`,
/// `all_resolvable=true` (benign) — distinct from `serve_ready=false`,
/// `all_resolvable=false` (a real listing-vs-resolver inconsistency).
#[derive(Clone, Debug)]
struct DoctorReport {
    repo_id: String,
    revision_id: String,
    tracks: Vec<DoctorTrack>,
    serve_ready: bool,
    all_resolvable: bool,
}

/// J7Q-05: build the composite [`DoctorReport`] from two distinct control reads.
///
/// The catalog listing (`status`) and per-track serve-time resolution (`current`)
/// read the same activation catalog through different code, so corroborating them
/// catches a listing-vs-resolver divergence that `readiness` (listing only) cannot
/// see.
fn build_doctor_report(
    client: &QuantaIndex,
    repo_id: RepoId,
    revision_id: RevisionId,
) -> CliResult<DoctorReport> {
    // Authoritative listing: which tracks the activation catalog serves for this
    // pair, each with its (generation, manifest_digest). Any transport / protocol
    // / remote failure here propagates fail-closed (the daemon is unreachable or
    // the catalog read failed — never a fabricated "healthy" verdict).
    let status = client
        .generations()
        .status(repo_id, revision_id)
        .map_err(map_sdk_error)?;
    let mut tracks = Vec::with_capacity(status.tracks.len());
    let mut all_resolvable = true;
    for record in &status.tracks {
        let resolution = client.generations().current(
            status.repo_id.clone(),
            status.revision_id.clone(),
            record.track,
        );
        // Classify the resolver's answer. A divergent snapshot or a typed
        // NOT_READY for a *listed* track is a recorded diagnosis finding
        // (`resolver_ok=false`), not a default — the failure is surfaced, never
        // masked. Every other error propagates fail-closed.
        let (resolver_ok, resolver_note) = match resolution {
            Ok(snapshot)
                if snapshot.manifest_generation == record.manifest_generation
                    && snapshot.manifest_digest == record.manifest_digest =>
            {
                (
                    true,
                    "resolved: catalog listing and serve-time resolver agree".to_string(),
                )
            }
            Ok(snapshot) => (
                false,
                format!(
                    "divergence: catalog lists generation={} digest={} but serve-time resolver returned generation={} digest={}",
                    record.manifest_generation.get(),
                    record.manifest_digest,
                    snapshot.manifest_generation.get(),
                    snapshot.manifest_digest
                ),
            ),
            Err(error) if is_not_ready(&error) => (
                false,
                "serve-time resolver returned NOT_READY for a track the catalog lists as active"
                    .to_string(),
            ),
            Err(error) => return Err(map_sdk_error(error)),
        };
        if !resolver_ok {
            all_resolvable = false;
        }
        tracks.push(DoctorTrack {
            track: record.track,
            manifest_generation: record.manifest_generation.get(),
            manifest_digest: record.manifest_digest.clone(),
            resolver_ok,
            resolver_note,
        });
    }
    let serve_ready = !tracks.is_empty() && all_resolvable;
    Ok(DoctorReport {
        repo_id: status.repo_id.as_str().to_string(),
        revision_id: status.revision_id.as_str().to_string(),
        tracks,
        serve_ready,
        all_resolvable,
    })
}

/// Whether a resolver error is the typed `NOT_READY` diagnosis state.
///
/// A `NOT_READY` for a *listed* track is a finding (the catalog listed it but the
/// resolver will not serve it), not a transport failure to abort on. Match the
/// typed code exactly; every other `SdkError` (including other remote codes) stays
/// an abort in [`build_doctor_report`].
fn is_not_ready(error: &SdkError) -> bool {
    matches!(
        error,
        SdkError::Remote {
            code: quanta_index_contract::SearchPlaneErrorCodeV2::NotReady,
            ..
        }
    )
}

/// J7Q-05: render a [`DoctorReport`].
///
/// `json` mode emits a stable machine-readable object (the field names are the
/// scriptable contract). `pretty` mode emits a line-oriented form. Both carry
/// the same verdict so automation and humans never reach different conclusions.
fn render_doctor(report: &DoctorReport, output: OutputMode) -> CliResult<String> {
    match output {
        OutputMode::Json => serde_json::to_string_pretty(&doctor_report_to_json(report))
            .map(|mut text| {
                text.push('\n');
                text
            })
            .map_err(|err| CliError::protocol(format!("failed to encode json output: {err}"))),
        OutputMode::Prometheus => Err(prometheus_is_metrics_only("doctor")),
        OutputMode::Pretty => {
            let mut rendered = String::new();
            fmt_ok(writeln!(rendered, "kind: doctor"))?;
            fmt_ok(writeln!(
                rendered,
                "generation_status: repo_id={} revision_id={}",
                report.repo_id, report.revision_id
            ))?;
            fmt_ok(writeln!(rendered, "serve_ready: {}", report.serve_ready))?;
            fmt_ok(writeln!(
                rendered,
                "all_resolvable: {}",
                report.all_resolvable
            ))?;
            if report.tracks.is_empty() {
                fmt_ok(writeln!(rendered, "tracks: 0 (none activated)"))?;
                return Ok(rendered);
            }
            fmt_ok(writeln!(rendered, "tracks: {}", report.tracks.len()))?;
            for (index, track) in report.tracks.iter().enumerate() {
                let display_index = index
                    .checked_add(1)
                    .ok_or_else(|| CliError::protocol("doctor track index overflow".to_string()))?;
                fmt_ok(writeln!(
                    rendered,
                    "{}. track={} manifest_generation={} manifest_digest={} resolver_ok={}",
                    display_index,
                    track.track.as_code_str(),
                    track.manifest_generation,
                    track.manifest_digest,
                    track.resolver_ok
                ))?;
                fmt_ok(writeln!(rendered, "   resolver: {}", track.resolver_note))?;
            }
            Ok(rendered)
        }
    }
}

/// Stable JSON shape for [`DoctorReport`].
///
/// Field names here are the scriptable contract — automation reads `serve_ready`
/// / `all_resolvable` / per-track `resolver_ok` directly. Built as a value (not a
/// serde-derived DTO) because it is a CLI-side composite of two wire DTOs, not
/// itself a wire DTO.
fn doctor_report_to_json(report: &DoctorReport) -> serde_json::Value {
    let tracks = report
        .tracks
        .iter()
        .map(|track| {
            serde_json::json!({
                "track": track.track.as_code_str(),
                "manifest_generation": track.manifest_generation,
                "manifest_digest": track.manifest_digest,
                "resolver_ok": track.resolver_ok,
                "resolver_note": track.resolver_note,
            })
        })
        .collect::<Vec<_>>();
    serde_json::json!({
        "kind": "doctor",
        "repo_id": report.repo_id,
        "revision_id": report.revision_id,
        "serve_ready": report.serve_ready,
        "all_resolvable": report.all_resolvable,
        "track_count": report.tracks.len(),
        "tracks": tracks,
    })
}

fn render_pretty(
    response: &SearchPlaneQueryIpcResponseEnvelope,
    rendered: &mut String,
) -> CliResult<()> {
    fmt_ok(writeln!(rendered, "request_id: {}", response.request_id))?;
    match &response.payload {
        SearchPlaneQueryIpcResponse::Text(payload) => {
            render_lexical_payload("lexical", payload, None, rendered)
        }
        SearchPlaneQueryIpcResponse::Symbol(payload) => render_symbol_payload(payload, rendered),
        SearchPlaneQueryIpcResponse::Semantic(payload) => render_lexical_payload(
            "semantic",
            &TextQueryResponse {
                generation: payload.generation.clone(),
                results: payload.results.clone(),
                window: payload.window.clone(),
                file_owner_rows: None,
                next_cursor: None,
            },
            Some(&payload.explanation),
            rendered,
        ),
        SearchPlaneQueryIpcResponse::Hybrid(payload) => render_hybrid_payload(payload, rendered),
        SearchPlaneQueryIpcResponse::HybridSeed(payload) => {
            render_hybrid_seed_payload(payload, rendered)
        }
        SearchPlaneQueryIpcResponse::Explain(payload) => {
            fmt_ok(writeln!(rendered, "kind: explain"))?;
            render_generation(&payload.generation, rendered)?;
            fmt_ok(writeln!(
                rendered,
                "presence: {}",
                payload.presence.as_str()
            ))?;
            render_explanation(&payload.explanation, rendered)?;
            Ok(())
        }
        SearchPlaneQueryIpcResponse::RepoMapQuery(payload) => {
            fmt_ok(writeln!(rendered, "kind: repomap"))?;
            fmt_ok(writeln!(
                rendered,
                "generation: repo_id={} revision_id={} manifest_generation={}",
                payload.repo_id.as_str(),
                payload.revision_id.as_str(),
                payload.manifest_generation.get()
            ))?;
            let meta = &payload.snapshot_meta;
            fmt_ok(writeln!(
                rendered,
                "snapshot: id={} projection_version={} authority_digest={}",
                meta.snapshot_id, meta.projection_version, meta.authority_digest
            ))?;
            fmt_ok(writeln!(
                rendered,
                "entries: {} dropped_entries_count={} drop_reason_codes={} degraded_reason_codes={}",
                payload.entries.len(),
                payload.dropped_entries_count,
                payload.drop_reason_codes.join(","),
                payload.degraded_reason_codes.join(",")
            ))?;
            for (index, entry) in payload.entries.iter().enumerate() {
                let display_index = index.checked_add(1).ok_or_else(|| {
                    CliError::protocol("repomap entry index overflow".to_string())
                })?;
                fmt_ok(writeln!(
                    rendered,
                    "{}. subject_identity={} doc_type={} kind={} owner_path={} rank={} score={} final_score_millis={}",
                    display_index,
                    entry.subject_identity,
                    entry.subject_doc_type.as_code_str(),
                    entry.subject_kind,
                    entry.owner_path,
                    entry.rank,
                    entry.score,
                    entry.final_score_millis
                ))?;
                fmt_ok(writeln!(
                    rendered,
                    "   authority: artifact_id={} digest={} projection_status={} redaction_state={}",
                    entry.projection_authority_artifact_id,
                    entry.projection_authority_digest,
                    entry.projection_status,
                    entry.redaction_state.as_code_str()
                ))?;
                if !entry.contributing_signals.is_empty() {
                    let pairs = entry
                        .contributing_signals
                        .iter()
                        .map(|(name, value)| format!("{name}={value}"))
                        .collect::<Vec<_>>()
                        .join(", ");
                    fmt_ok(writeln!(rendered, "   contributing_signals: {pairs}"))?;
                }
            }
            Ok(())
        }
        SearchPlaneQueryIpcResponse::Error(error) => Err(CliError::remote(
            render_remote_error_text(&error.code, &error.message, error.repair.as_ref())?,
        )),
        SearchPlaneQueryIpcResponse::RuntimeMetadata(payload) => {
            render_runtime_metadata_payload(payload, rendered)
        }
        SearchPlaneQueryIpcResponse::History(payload) => render_history_payload(payload, rendered),
        SearchPlaneQueryIpcResponse::Structural(payload) => {
            render_structural_payload(payload, rendered)
        }
        SearchPlaneQueryIpcResponse::ClusterMembershipRead(_) => Err(CliError::protocol(
            "ClusterMembershipRead is an SDK authority response and has no searchctl command"
                .to_string(),
        )),
    }
}

/// Render a hybrid response (QI-BB-022).
///
/// One line per fused row: the lane row as a lexical hit, then the RRF
/// score and each lane's rank and raw score.
fn render_hybrid_payload(payload: &HybridQueryResponse, rendered: &mut String) -> CliResult<()> {
    fmt_ok(writeln!(rendered, "kind: hybrid"))?;
    render_generation(&payload.generation, rendered)?;
    fmt_ok(writeln!(rendered, "results: {}", payload.results.len()))?;
    render_window_line(&payload.window, rendered)?;
    for (index, row) in payload.results.iter().enumerate() {
        let display_index = index
            .checked_add(1)
            .ok_or_else(|| CliError::protocol("candidate index overflow".to_string()))?;
        let lanes = row
            .contributions
            .iter()
            .map(|contribution| {
                format!(
                    "{}#{}({})",
                    contribution.lane.as_code_str(),
                    contribution.rank,
                    contribution.raw_score
                )
            })
            .collect::<Vec<_>>()
            .join(" ");
        fmt_ok(writeln!(
            rendered,
            "{}. candidate_id={} path={} lines={}-{} score={} fused={} lanes={}",
            display_index,
            row.candidate.candidate_id,
            row.candidate.repo_relative_path.as_str(),
            row.candidate.start_line,
            row.candidate.end_line,
            row.candidate.score,
            row.fused_score,
            lanes
        ))?;
        for line in row.candidate.snippet.lines() {
            fmt_ok(writeln!(rendered, "   {line}"))?;
        }
    }
    render_explanation(&payload.explanation, rendered)?;
    Ok(())
}

/// Render the one canonical seed list (QI-BB-019): each seed with its
/// typed identity and the lane contributions that ranked it.
fn render_hybrid_seed_payload(
    payload: &HybridSeedQueryResponse,
    rendered: &mut String,
) -> CliResult<()> {
    fmt_ok(writeln!(rendered, "kind: hybrid-seed"))?;
    render_generation(&payload.generation, rendered)?;
    fmt_ok(writeln!(
        rendered,
        "manifest_digest: {} seeds: {} has_more: {}",
        payload.manifest_digest,
        payload.window.returned(),
        display_has_more(payload.window.has_more())
    ))?;
    for seed in &payload.seed_candidates {
        let contributions = seed
            .contributions
            .iter()
            .map(|contribution| {
                let lane = match contribution.lane {
                    quanta_index_contract::SeedLane::Exact => "exact",
                    quanta_index_contract::SeedLane::Bm25 => "bm25",
                    quanta_index_contract::SeedLane::Dense => "dense",
                };
                contribution.corpus_kind.map_or_else(
                    || format!("{lane}#{}", contribution.rank),
                    |corpus| format!("{lane}#{}@{}", contribution.rank, corpus.as_code_str()),
                )
            })
            .collect::<Vec<_>>()
            .join(",");
        fmt_ok(writeln!(
            rendered,
            "{}. entity={} owner_kind={} path={} lanes={}{}",
            seed.seed_rank,
            seed.entity_id,
            seed.owner_kind.as_code_str(),
            seed.repo_relative_path.as_str(),
            contributions,
            if seed.degraded_reasons.is_empty() {
                String::new()
            } else {
                format!(" degraded={}", seed.degraded_reasons.join(","))
            }
        ))?;
        for line in seed.snippet.lines().take(3) {
            fmt_ok(writeln!(rendered, "   {line}"))?;
        }
    }
    render_explanation(&payload.explanation, rendered)?;
    Ok(())
}

fn render_history_payload(
    payload: &SearchPlaneHistoryQueryResponse,
    rendered: &mut String,
) -> CliResult<()> {
    fmt_ok(writeln!(rendered, "kind: history"))?;
    render_generation(&payload.generation, rendered)?;
    fmt_ok(writeln!(
        rendered,
        "commits: {} diffs: {}",
        payload.commits.len(),
        payload.diffs.len()
    ))?;
    let matched = match payload.window.candidate_count() {
        quanta_index_contract::CandidateCountV1::Exact(count) => format!("{count}"),
        quanta_index_contract::CandidateCountV1::AtLeast(count) => format!(">={count}"),
    };
    fmt_ok(writeln!(
        rendered,
        "order: {} matched: {matched} examined: {} has_more: {}",
        payload.order,
        payload.examined,
        display_has_more(payload.window.has_more())
    ))?;
    render_read_epoch_line(payload.read_epoch, rendered)?;
    render_continuation_cursor(payload.next_cursor.as_ref(), rendered)?;
    for (index, commit) in payload.commits.iter().enumerate() {
        let display_index = index
            .checked_add(1)
            .ok_or_else(|| CliError::protocol("commit index overflow".to_string()))?;
        fmt_ok(writeln!(
            rendered,
            "{}.{} sha={} author={} committer={} committed_at_unix_s={} is_merge={} tags={}",
            display_index,
            render_history_score(commit.score),
            commit.sha.to_hex(),
            commit.author,
            commit.committer,
            commit.committed_at_unix_s,
            commit.is_merge,
            commit.tags.join(",")
        ))?;
        for line in commit.message.lines() {
            fmt_ok(writeln!(rendered, "   {line}"))?;
        }
    }
    for (index, diff) in payload.diffs.iter().enumerate() {
        let display_index = index
            .checked_add(1)
            .ok_or_else(|| CliError::protocol("diff index overflow".to_string()))?;
        fmt_ok(writeln!(
            rendered,
            "{}.{} path={} hunk_header={} side={} lines={}-{}",
            display_index,
            render_history_score(diff.score),
            diff.repo_relative_path,
            diff.hunk_header,
            diff.side.as_str(),
            diff.line_start,
            diff.line_end
        ))?;
        for line in diff.snippet.lines() {
            fmt_ok(writeln!(rendered, "   {line}"))?;
        }
    }
    Ok(())
}

/// ` score=<s>` for a scored row (a relevance page), nothing otherwise.
fn render_history_score(score: Option<quanta_index_contract::HistoryScoreV1>) -> String {
    score.map_or_else(String::new, |score| format!(" score={score}"))
}

/// One line for a page's window: how many candidates matched (exactly, or
/// at least) and whether the page cut them (QI-BB-025).
fn display_has_more(value: Option<bool>) -> &'static str {
    match value {
        Some(true) => "true",
        Some(false) => "false",
        None => "unknown",
    }
}

fn render_window_line(window: &QueryResultWindowV2, rendered: &mut String) -> CliResult<()> {
    let matched = match window.candidate_count() {
        quanta_index_contract::CandidateCountV1::Exact(count) => format!("{count}"),
        quanta_index_contract::CandidateCountV1::AtLeast(count) => format!(">={count}"),
    };
    fmt_ok(writeln!(
        rendered,
        "matched: {matched} has_more: {}",
        display_has_more(window.has_more())
    ))
}

/// One line for the auxiliary authority epoch a page was cut from
/// (QI-BB-020 W2); JSON output carries it as `read_epoch`.
fn render_read_epoch_line(epoch: AuxEpochV1, rendered: &mut String) -> CliResult<()> {
    fmt_ok(writeln!(rendered, "epoch: {epoch}"))
}

/// One line for a keyset page's order, window and work (QI-BB-025 W4),
/// in the shape the history page prints: `order: candidate_id matched: N
/// examined: M has_more: B`.
fn render_keyset_page_line(
    window: &QueryResultWindowV2,
    examined: u64,
    rendered: &mut String,
) -> CliResult<()> {
    let matched = match window.candidate_count() {
        quanta_index_contract::CandidateCountV1::Exact(count) => format!("{count}"),
        quanta_index_contract::CandidateCountV1::AtLeast(count) => format!(">={count}"),
    };
    fmt_ok(writeln!(
        rendered,
        "order: candidate_id matched: {matched} examined: {examined} has_more: {}",
        display_has_more(window.has_more())
    ))
}

fn render_structural_payload(
    payload: &SearchPlaneStructuralQueryResponse,
    rendered: &mut String,
) -> CliResult<()> {
    fmt_ok(writeln!(rendered, "kind: structural"))?;
    render_generation(&payload.generation, rendered)?;
    fmt_ok(writeln!(rendered, "results: {}", payload.results.len()))?;
    render_keyset_page_line(&payload.window, payload.examined, rendered)?;
    render_read_epoch_line(payload.read_epoch, rendered)?;
    render_continuation_cursor(payload.next_cursor.as_ref(), rendered)?;
    for (index, candidate) in payload.results.iter().enumerate() {
        let display_index = index
            .checked_add(1)
            .ok_or_else(|| CliError::protocol("structural candidate index overflow".to_string()))?;
        fmt_ok(writeln!(
            rendered,
            "{}. candidate_id={} bindings={}",
            display_index,
            candidate.candidate_id,
            candidate.bindings.len()
        ))?;
        for binding in &candidate.bindings {
            fmt_ok(writeln!(
                rendered,
                "   {}: bytes={}-{} lines={}-{}",
                binding.metavariable,
                binding.start_byte,
                binding.end_byte,
                binding.start_line,
                binding.end_line
            ))?;
        }
    }
    Ok(())
}

fn render_lexical_payload(
    kind: &str,
    payload: &TextQueryResponse,
    explanation: Option<&SearchExplanation>,
    rendered: &mut String,
) -> CliResult<()> {
    fmt_ok(writeln!(rendered, "kind: {kind}"))?;
    render_generation(&payload.generation, rendered)?;
    fmt_ok(writeln!(rendered, "results: {}", payload.results.len()))?;
    for (index, candidate) in payload.results.iter().enumerate() {
        let display_index = index
            .checked_add(1)
            .ok_or_else(|| CliError::protocol("candidate index overflow".to_string()))?;
        fmt_ok(writeln!(
            rendered,
            "{}. candidate_id={} path={} lines={}-{} score={}",
            display_index,
            candidate.candidate_id,
            candidate.repo_relative_path.as_str(),
            candidate.start_line,
            candidate.end_line,
            candidate.score
        ))?;
        for line in candidate.snippet.lines() {
            fmt_ok(writeln!(rendered, "   {line}"))?;
        }
    }
    if let Some(file_owner_rows) = &payload.file_owner_rows {
        fmt_ok(writeln!(
            rendered,
            "file_owner_rows: {}",
            file_owner_rows.len()
        ))?;
        for (index, row) in file_owner_rows.iter().enumerate() {
            let display_index = index.checked_add(1).ok_or_else(|| {
                CliError::protocol("file owner projection index overflow".to_string())
            })?;
            let owners = if row.owners.is_empty() {
                "-".to_string()
            } else {
                row.owners.join(",")
            };
            fmt_ok(writeln!(
                rendered,
                "owner_row {}. candidate_id={} path={} owners={}",
                display_index,
                row.candidate_id,
                row.repo_relative_path.as_str(),
                owners
            ))?;
        }
    }
    render_continuation_cursor(payload.next_cursor.as_ref(), rendered)?;
    if let Some(explanation) = explanation {
        render_explanation(explanation, rendered)?;
    }
    Ok(())
}

fn render_symbol_payload(payload: &SymbolQueryResponse, rendered: &mut String) -> CliResult<()> {
    fmt_ok(writeln!(rendered, "kind: symbol"))?;
    render_generation(&payload.generation, rendered)?;
    fmt_ok(writeln!(rendered, "results: {}", payload.results.len()))?;
    for (index, candidate) in payload.results.iter().enumerate() {
        let display_index = index
            .checked_add(1)
            .ok_or_else(|| CliError::protocol("symbol candidate index overflow".to_string()))?;
        render_symbol_candidate(display_index, candidate, rendered)?;
    }
    render_continuation_cursor(payload.next_cursor.as_ref(), rendered)
}

/// The continuation of a ranked page, as `--cursor-json` takes it back.
fn render_continuation_cursor(
    cursor: Option<&ContinuationTokenV2>,
    rendered: &mut String,
) -> CliResult<()> {
    if let Some(cursor) = cursor {
        let json = serde_json::to_string(cursor)
            .map_err(|err| CliError::protocol(format!("encode next_cursor: {err}")))?;
        fmt_ok(writeln!(rendered, "next_cursor: {json}"))?;
    }
    Ok(())
}

fn render_symbol_candidate(
    display_index: usize,
    candidate: &SymbolCandidate,
    rendered: &mut String,
) -> CliResult<()> {
    let family = candidate.symbol_kind_family.map_or(
        "-",
        quanta_index_contract::lex::SymbolKindFamily::as_code_str,
    );
    fmt_ok(writeln!(
        rendered,
        "{}. candidate_id={} path={} lines={}-{} score={} symbol_kind={} symbol_kind_family={}",
        display_index,
        candidate.candidate_id,
        candidate.repo_relative_path.as_str(),
        candidate.start_line,
        candidate.end_line,
        candidate.score,
        candidate.symbol_kind.as_str(),
        family,
    ))?;
    for line in candidate.snippet.lines() {
        fmt_ok(writeln!(rendered, "   {line}"))?;
    }
    Ok(())
}

fn render_runtime_metadata_payload(
    payload: &SearchPlaneRuntimeMetadataQueryResponse,
    rendered: &mut String,
) -> CliResult<()> {
    fmt_ok(writeln!(rendered, "kind: runtime-metadata"))?;
    render_generation(&payload.generation, rendered)?;
    fmt_ok(writeln!(rendered, "results: {}", payload.results.len()))?;
    render_keyset_page_line(&payload.window, payload.examined, rendered)?;
    render_read_epoch_line(payload.read_epoch, rendered)?;
    fmt_ok(writeln!(
        rendered,
        "universe_epoch: {}",
        payload.universe_epoch
    ))?;
    render_continuation_cursor(payload.next_cursor.as_ref(), rendered)?;
    for (index, candidate) in payload.results.iter().enumerate() {
        let display_index = index.checked_add(1).ok_or_else(|| {
            CliError::protocol("runtime-metadata candidate index overflow".to_string())
        })?;
        fmt_ok(writeln!(
            rendered,
            "{}. candidate_id={} path={} lines={}-{} score={}",
            display_index,
            candidate.candidate_id,
            candidate.repo_relative_path.as_str(),
            candidate.start_line,
            candidate.end_line,
            candidate.score
        ))?;
        for line in candidate.snippet.lines() {
            fmt_ok(writeln!(rendered, "   {line}"))?;
        }
    }
    Ok(())
}

fn render_generation(generation: &GenerationPin, rendered: &mut String) -> CliResult<()> {
    fmt_ok(writeln!(
        rendered,
        "generation: repo_id={} revision_id={} manifest_generation={}",
        generation.repo_id.as_str(),
        generation.revision_id.as_str(),
        generation.manifest_generation.get()
    ))
}

fn render_explanation(explanation: &SearchExplanation, rendered: &mut String) -> CliResult<()> {
    fmt_ok(writeln!(rendered, "summary: {}", explanation.summary))?;
    fmt_ok(writeln!(rendered, "strategy: {}", explanation.strategy))?;
    let touched = explanation
        .engines_touched
        .iter()
        .map(engine_name)
        .collect::<Vec<_>>()
        .join(",");
    fmt_ok(writeln!(rendered, "engines_touched: {touched}"))?;
    if let Some(reason) = explanation.early_stop_reason {
        fmt_ok(writeln!(
            rendered,
            "early_stop_reason: {}",
            early_stop_name(reason)
        ))?;
    }
    fmt_ok(writeln!(
        rendered,
        "ranker_weights_hash: {}",
        encode_hex(&explanation.ranker_weights_hash)?
    ))?;
    if !explanation.planner_trace.is_empty() {
        fmt_ok(writeln!(rendered, "planner_trace:"))?;
        for PlannerTraceEntry { stage, detail } in &explanation.planner_trace {
            fmt_ok(writeln!(
                rendered,
                "  - {}: {}",
                planner_stage_name(*stage),
                detail
            ))?;
        }
    }
    if !explanation.contributions.is_empty() {
        fmt_ok(writeln!(rendered, "contributions:"))?;
        for contribution in &explanation.contributions {
            fmt_ok(writeln!(
                rendered,
                "  - {} signal_value={} weight={} contribution={}",
                contribution.signal_name,
                contribution.signal_value,
                contribution.weight,
                contribution.contribution
            ))?;
        }
    }
    Ok(())
}

fn response_kind_name(response: &SearchPlaneQueryIpcResponse) -> &'static str {
    match response {
        SearchPlaneQueryIpcResponse::Text(_) => "Text",
        SearchPlaneQueryIpcResponse::Symbol(_) => "Symbol",
        SearchPlaneQueryIpcResponse::Semantic(_) => "Semantic",
        SearchPlaneQueryIpcResponse::Hybrid(_) => "Hybrid",
        SearchPlaneQueryIpcResponse::HybridSeed(_) => "HybridSeed",
        SearchPlaneQueryIpcResponse::History(_) => "History",
        SearchPlaneQueryIpcResponse::Structural(_) => "Structural",
        SearchPlaneQueryIpcResponse::RepoMapQuery(_) => "RepoMapQuery",
        SearchPlaneQueryIpcResponse::Explain(_) => "Explain",
        SearchPlaneQueryIpcResponse::Error(_) => "Error",
        SearchPlaneQueryIpcResponse::RuntimeMetadata(_) => "RuntimeMetadata",
        SearchPlaneQueryIpcResponse::ClusterMembershipRead(_) => "ClusterMembershipRead",
    }
}

fn command_kind_name(kind: CommandKind) -> &'static str {
    match kind {
        CommandKind::Lexical => "lexical",
        CommandKind::Symbol => "symbol",
        CommandKind::Semantic => "semantic",
        CommandKind::Hybrid => "hybrid",
        CommandKind::HybridSeed => "hybrid-seed",
        CommandKind::Explain => "explain",
        CommandKind::RepoMap => "repomap",
        CommandKind::RuntimeMetadata => "runtime-metadata",
        CommandKind::History => "history",
        CommandKind::Structural => "structural",
        CommandKind::Readiness => "readiness",
        CommandKind::Doctor => "doctor",
        CommandKind::Metrics => "metrics",
        CommandKind::Quarantine => "quarantine",
    }
}

fn planner_stage_name(stage: quanta_index_contract::PlannerStage) -> &'static str {
    stage.as_str()
}

fn engine_name(engine: &EngineTouched) -> &'static str {
    engine.as_str()
}

fn early_stop_name(reason: EarlyStopReason) -> &'static str {
    reason.as_str()
}

fn encode_hex(bytes: &[u8]) -> CliResult<String> {
    let mut out = String::with_capacity(bytes.len().saturating_mul(2));
    for byte in bytes {
        out.push(hex_digit(*byte >> 4)?);
        out.push(hex_digit(*byte & 0x0f)?);
    }
    Ok(out)
}

fn hex_digit(nibble: u8) -> CliResult<char> {
    match nibble {
        0 => Ok('0'),
        1 => Ok('1'),
        2 => Ok('2'),
        3 => Ok('3'),
        4 => Ok('4'),
        5 => Ok('5'),
        6 => Ok('6'),
        7 => Ok('7'),
        8 => Ok('8'),
        9 => Ok('9'),
        10 => Ok('a'),
        11 => Ok('b'),
        12 => Ok('c'),
        13 => Ok('d'),
        14 => Ok('e'),
        15 => Ok('f'),
        _ => Err(CliError::protocol(format!(
            "hex nibble out of range: {nibble}"
        ))),
    }
}

fn fmt_ok(result: std::fmt::Result) -> CliResult<()> {
    result.map_err(|_err| CliError::protocol("string formatting failed".to_string()))
}

/// Render a typed remote error for the pretty CLI path (J7Q-06).
///
/// `code: message` stays the headline; when the wire carried typed repair
/// metadata it is appended as a distinct, scriptable hint block (class, the
/// supported alternative shapes, and the docs anchor). One renderer serves both
/// the wire-error and the `SdkError::Remote` arms so the guidance shape cannot
/// drift between them. The JSON path needs no special handling — it serializes
/// the whole envelope, `repair` included. This only renders guidance; it never
/// rewrites the query or softens the failure.
fn render_remote_error_text(
    code: &quanta_index_contract::SearchPlaneErrorCodeV2,
    message: &str,
    repair: Option<&QueryErrorRepair>,
) -> CliResult<String> {
    let mut text = String::new();
    fmt_ok(write!(text, "{code}: {message}"))?;
    if let Some(repair) = repair {
        fmt_ok(write!(
            text,
            "\n  repair class: {}",
            repair.class.as_code_str()
        ))?;
        if !repair.supported_alternatives.is_empty() {
            fmt_ok(write!(
                text,
                "\n  try: {}",
                repair.supported_alternatives.join(" | ")
            ))?;
        }
        if let Some(anchor) = &repair.docs_anchor {
            fmt_ok(write!(text, "\n  docs: {anchor}"))?;
        }
    }
    Ok(text)
}

fn usage() -> &'static str {
    "\
quanta-index-searchctl

Global flags:
  --socket PATH
  --state-root PATH
  --output pretty|json|prometheus   (prometheus: `metrics` only)

Read-only subcommands:
  lexical          --repo-id ID --revision-id REV --manifest-generation N --syntax native|sourcegraph --query-text TEXT --top-k N [--cursor-json PATH|-]
  symbol           --repo-id ID --revision-id REV --manifest-generation N --syntax native|sourcegraph --query-text TEXT --top-k N [--cursor-json PATH|-]
  semantic         --repo-id ID --revision-id REV --manifest-generation N --query-text TEXT --top-k N [--scope-query TEXT --scope-syntax native|sourcegraph --scope-top-k N]
  hybrid           --repo-id ID --revision-id REV --manifest-generation N --syntax native|sourcegraph --query-text TEXT --semantic-query-text TEXT --top-k N
  hybrid-seed      --repo-id ID --revision-id REV --manifest-generation N --lexical-query TEXT --lexical-syntax native|sourcegraph --semantic-query TEXT --top-k N
  explain          --repo-id ID --revision-id REV --manifest-generation N --candidate-json PATH|- [--syntax native|sourcegraph --query-text TEXT]
  explain          --repo-id ID --revision-id REV --manifest-generation N --hybrid-candidate-json PATH|- --syntax native|sourcegraph --query-text TEXT --semantic-query-text TEXT --top-k N
  repomap          --repo-id ID --revision-id REV --manifest-generation N --query-text TEXT --top-k N --token-budget N [--focus-subject subject_identity:subject_doc_type]
    history          --repo-id ID --revision-id REV --manifest-generation N --syntax native|sourcegraph --query-text TEXT --top-k N --order recency|relevance, history          --repo-id ID --revision-id REV --manifest-generation N --syntax native|sourcegraph --query-text TEXT --top-k N [--cursor-json PATH|-], runtime-metadata --repo-id ID --revision-id REV --manifest-generation N --syntax native|sourcegraph --query-text TEXT --top-k N, runtime-metadata --repo-id ID --revision-id REV --manifest-generation N --syntax native|sourcegraph --query-text TEXT --top-k N [--cursor-json PATH|-], structural       --repo-id ID --revision-id REV --manifest-generation N --syntax native|sourcegraph --query-text TEXT --top-k N, structural       --repo-id ID --revision-id REV --manifest-generation N --syntax native|sourcegraph --query-text TEXT --top-k N [--cursor-json PATH|-],
  readiness        --repo-id ID --revision-id REV
  doctor           --repo-id ID --revision-id REV
  metrics
  quarantine       list
  quarantine       discard --track lexical|semantic --path PATH --reason CODE [--detail TEXT]
  quarantine       discard --repomap-file NAME --reason TEXT

`hybrid` runs two independent, bounded lanes over the pinned generation — the
lexical lane over `--query-text` and the dense lane over the embedded
`--semantic-query-text` — and fuses their union by reciprocal rank fusion, so a
document with no lexical overlap can enter the page on dense relevance alone.
Each row carries its fused score and the rank and raw score every lane gave it.
`semantic --scope-query` is the other shape: the dense lane is confined to the
lexical universe the scope query matches (a lexical-scoped rerank), so a
document the scope does not match can never enter its page.

`explain --hybrid-candidate-json` re-derives a hybrid row against the index
under both queries it was fused for: the lexical lane through the engine's own
score trace, the dense lane by scoring the row's stored vector against the
embedded `--semantic-query-text`, and the fusion by re-running both lanes at
`--top-k` (the hybrid's fused top_k). The trace reports
`explain.score_reconciled`, `explain.dense_reconciled` and
`explain.fused_reconciled`; a row whose carried provenance the index does not
reproduce fails on that axis.

`doctor` is the composite read-only diagnosis: it fuses the activation-catalog
listing with per-track serve-time resolution and reports a machine-readable
verdict (serve_ready, all_resolvable, per-track resolver_ok). It exits 0 when the
diagnosis completes (read the JSON verdict fields for the health conclusion) and
uses the standard transport/usage/remote/protocol exit codes only for failures
to *reach* a verdict.

`metrics` scrapes every counter, gauge and histogram the daemon has aggregated
since it started (route latency, outcomes and interruptions, socket admission,
queue wait and in-flight slots, response bytes, examined candidates, caches,
provider retries and failures, generation disk bytes, the process resident
set, the maintenance timer, boot inventory) over the control socket;
`--output prometheus` emits the text exposition format for a scraper.

Operator scrape path: the daemon listens on a private Unix socket, not an
HTTP port, so a Prometheus server does not scrape it directly. Run this
command from the daemon's user on a schedule and write its output where the
node_exporter textfile collector reads it, e.g.

  quanta-index-searchctl --state-root /var/lib/quanta-index metrics \
    --output prometheus > /var/lib/node_exporter/textfile/quanta_index.prom.tmp \
    && mv /var/lib/node_exporter/textfile/quanta_index.prom.tmp \
          /var/lib/node_exporter/textfile/quanta_index.prom

(node_exporter must run with `--collector.textfile.directory` pointing at
that directory; the temporary-file-then-rename keeps a half-written scrape
from being read). Every metric is process-wide and label-free by design:
no repository, generation or query text ever becomes a label.

`quarantine list` prints what the daemon has set aside as it cannot trust it
(a generation directory with an unreadable or foreign identity, a RepoMap file
that does not decode), each line in the form `quarantine discard` takes back.
A discard names one entry exactly as listed; the daemon re-checks before it
removes anything and refuses a stale or invented entry typed.
"
}

type CliResult<T> = Result<T, CliError>;

#[derive(Debug)]
struct CliError {
    exit_code: u8,
    message: String,
}

impl CliError {
    fn usage(message: String) -> Self {
        Self {
            exit_code: EXIT_USAGE,
            message,
        }
    }

    fn transport(message: String) -> Self {
        Self {
            exit_code: EXIT_TRANSPORT,
            message,
        }
    }

    fn remote(message: String) -> Self {
        Self {
            exit_code: EXIT_REMOTE,
            message,
        }
    }

    fn protocol(message: String) -> Self {
        Self {
            exit_code: EXIT_PROTOCOL,
            message,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_missing_scope_syntax() {
        let parsed = ParsedCommand::parse([
            "semantic",
            "--repo-id",
            "repo",
            "--revision-id",
            "rev",
            "--manifest-generation",
            "7",
            "--query-text",
            "embedding query",
            "--top-k",
            "5",
            "--scope-query",
            "lang:rust",
        ]);
        assert!(parsed.is_err());
        let Err(error) = parsed else {
            return;
        };
        assert_eq!(error.exit_code, EXIT_USAGE);
        assert!(error.message.contains("--scope-syntax"));
    }

    #[test]
    fn parses_repomap_focus_subject() {
        let parsed = parse_focus_subject("subject-1:file");
        assert!(parsed.is_ok());
        let Ok(focus) = parsed else {
            return;
        };
        assert_eq!(focus.subject_identity, "subject-1");
        assert_eq!(focus.subject_doc_type, RepoMapDocType::File);
    }

    #[test]
    fn parses_semantic_query_text() {
        let parsed = ParsedCommand::parse([
            "semantic",
            "--repo-id",
            "repo",
            "--revision-id",
            "rev",
            "--manifest-generation",
            "7",
            "--query-text",
            "1 0 2.5",
            "--top-k",
            "5",
        ]);
        assert!(parsed.is_ok());
        let Ok(parsed) = parsed else {
            return;
        };
        let CliRequest::Semantic(request) = parsed.request else {
            panic!("expected semantic payload");
        };
        assert_eq!(request.query_text, "1 0 2.5".to_string());
    }

    #[test]
    fn parses_hybrid_semantic_query_text() {
        let parsed = ParsedCommand::parse([
            "hybrid-seed",
            "--repo-id",
            "repo",
            "--revision-id",
            "rev",
            "--manifest-generation",
            "7",
            "--lexical-query",
            "needle",
            "--lexical-syntax",
            "native",
            "--semantic-query",
            "1 0 2.5",
            "--top-k",
            "5",
        ]);
        assert!(parsed.is_ok());
        let Ok(parsed) = parsed else {
            return;
        };
        let CliRequest::HybridSeed(request) = parsed.request else {
            panic!("expected hybrid-seed payload");
        };
        assert_eq!(request.semantic_query_text, "1 0 2.5".to_string());
    }

    #[test]
    fn rejects_legacy_semantic_query_vector_flag() {
        let parsed = ParsedCommand::parse([
            "semantic",
            "--repo-id",
            "repo",
            "--revision-id",
            "rev",
            "--manifest-generation",
            "7",
            "--query-vector",
            "1,2,3",
            "--top-k",
            "5",
        ]);
        assert!(parsed.is_err());
        let Err(error) = parsed else {
            return;
        };
        assert_eq!(error.exit_code, EXIT_USAGE);
        assert!(error.message.contains("--query-vector"));
    }

    #[test]
    fn rejects_legacy_semantic_query_handle_flag() {
        let parsed = ParsedCommand::parse([
            "semantic",
            "--repo-id",
            "repo",
            "--revision-id",
            "rev",
            "--manifest-generation",
            "7",
            "--query-vector-handle",
            "emb-123",
            "--top-k",
            "5",
        ]);
        assert!(parsed.is_err());
        let Err(error) = parsed else {
            return;
        };
        assert_eq!(error.exit_code, EXIT_USAGE);
        assert!(error.message.contains("--query-vector-handle"));
    }

    #[test]
    fn rejects_legacy_hybrid_semantic_handle_flag() {
        let parsed = ParsedCommand::parse([
            "hybrid-seed",
            "--repo-id",
            "repo",
            "--revision-id",
            "rev",
            "--manifest-generation",
            "7",
            "--lexical-query",
            "needle",
            "--lexical-syntax",
            "native",
            "--semantic-vector-handle",
            "emb-456",
            "--top-k",
            "5",
        ]);
        assert!(parsed.is_err());
        let Err(error) = parsed else {
            return;
        };
        assert_eq!(error.exit_code, EXIT_USAGE);
        assert!(error.message.contains("--semantic-vector-handle"));
    }

    // QI-BB-018: the true-hybrid route is reachable from the operator
    // surface, with the two lanes' queries and the fused top_k.
    #[test]
    fn parses_hybrid_subcommand_into_a_hybrid_request() {
        let parsed = ParsedCommand::parse([
            "hybrid",
            "--repo-id",
            "repo",
            "--revision-id",
            "rev",
            "--manifest-generation",
            "7",
            "--syntax",
            "sourcegraph",
            "--query-text",
            "needle",
            "--semantic-query-text",
            "where the needle is kept",
            "--top-k",
            "5",
        ]);
        assert!(parsed.is_ok(), "{parsed:?}");
        let Ok(parsed) = parsed else {
            return;
        };
        assert_eq!(parsed.kind, CommandKind::Hybrid);
        let CliRequest::Hybrid(request) = parsed.request else {
            panic!("expected hybrid payload");
        };
        assert_eq!(request.text_query.syntax, TextQuerySyntax::Sourcegraph);
        assert_eq!(request.text_query.query_text, "needle");
        assert_eq!(request.semantic_query_text, "where the needle is kept");
        assert_eq!(request.top_k, 5);
        assert_eq!(request.text_query.top_k, 5);
        assert_eq!(
            request.generation,
            Some(GenerationPin::new(
                RepoId::new("repo").expect("static fixture ID satisfies canonical policy"),
                RevisionId::new("rev").expect("static fixture ID satisfies canonical policy"),
                ManifestGeneration::new(7)
            ))
        );
        for (missing, flag) in [
            (
                vec![
                    "--syntax",
                    "native",
                    "--query-text",
                    "needle",
                    "--top-k",
                    "5",
                ],
                "--semantic-query-text",
            ),
            (
                vec![
                    "--syntax",
                    "native",
                    "--semantic-query-text",
                    "x",
                    "--top-k",
                    "5",
                ],
                "--query-text",
            ),
            (
                vec![
                    "--query-text",
                    "needle",
                    "--semantic-query-text",
                    "x",
                    "--top-k",
                    "5",
                ],
                "--syntax",
            ),
            (
                vec![
                    "--syntax",
                    "native",
                    "--query-text",
                    "needle",
                    "--semantic-query-text",
                    "x",
                ],
                "--top-k",
            ),
        ] {
            let mut args = vec![
                "hybrid",
                "--repo-id",
                "repo",
                "--revision-id",
                "rev",
                "--manifest-generation",
                "7",
            ];
            args.extend(missing);
            let refused = ParsedCommand::parse(args);
            let Err(error) = refused else {
                panic!("hybrid without {flag} must be refused");
            };
            assert_eq!(error.exit_code, EXIT_USAGE);
            assert!(error.message.contains(flag), "{}", error.message);
        }
    }

    #[test]
    fn parses_lexical_sourcegraph_query_request() {
        let parsed = ParsedCommand::parse([
            "lexical",
            "--repo-id",
            "repo",
            "--revision-id",
            "rev",
            "--manifest-generation",
            "7",
            "--syntax",
            "sourcegraph",
            "--query-text",
            "repo:repo lang:rust needle",
            "--top-k",
            "11",
        ]);
        assert!(parsed.is_ok());
        let Ok(parsed) = parsed else {
            return;
        };
        let CliRequest::Lexical(request) = parsed.request else {
            panic!("expected lexical text payload");
        };
        assert_eq!(request.syntax, TextQuerySyntax::Sourcegraph);
        assert_eq!(request.query_text.as_str(), "repo:repo lang:rust needle");
        assert_eq!(request.top_k, 11);
        assert_eq!(
            request.generation.map(|pin| pin.manifest_generation.get()),
            Some(7)
        );
    }

    #[test]
    fn explicit_socket_override_builds_sdk_connect_options() {
        let parsed = ParsedCommand::parse([
            "--socket",
            "/tmp/quanta/query.sock",
            "lexical",
            "--repo-id",
            "repo",
            "--revision-id",
            "rev",
            "--manifest-generation",
            "7",
            "--syntax",
            "native",
            "--query-text",
            "needle",
            "--top-k",
            "3",
        ]);
        assert!(parsed.is_ok());
        let Ok(parsed) = parsed else {
            return;
        };
        assert_eq!(
            parsed.connect_options,
            ConnectOptions::default()
                .with_query_socket("/tmp/quanta/query.sock")
                .with_control_socket("/tmp/quanta/control.sock")
                .with_ingest_socket("/tmp/quanta/ingest.sock")
        );
    }

    #[test]
    fn parses_symbol_query_request() {
        let parsed = ParsedCommand::parse([
            "symbol",
            "--repo-id",
            "repo",
            "--revision-id",
            "rev",
            "--manifest-generation",
            "7",
            "--syntax",
            "native",
            "--query-text",
            "MySymbol",
            "--top-k",
            "5",
        ]);
        assert!(parsed.is_ok());
        let Ok(parsed) = parsed else {
            return;
        };
        let CliRequest::Symbol(request) = parsed.request else {
            panic!("expected symbol payload");
        };
        assert_eq!(request.syntax, TextQuerySyntax::Native);
        assert_eq!(request.query_text.as_str(), "MySymbol");
        assert_eq!(request.top_k, 5);
        assert_eq!(
            request.generation.map(|pin| pin.manifest_generation.get()),
            Some(7)
        );
    }

    #[test]
    fn rejects_symbol_missing_top_k() {
        let parsed = ParsedCommand::parse([
            "symbol",
            "--repo-id",
            "repo",
            "--revision-id",
            "rev",
            "--manifest-generation",
            "7",
            "--syntax",
            "native",
            "--query-text",
            "MySymbol",
        ]);
        assert!(parsed.is_err());
        let Err(error) = parsed else {
            return;
        };
        assert_eq!(error.exit_code, EXIT_USAGE);
        assert!(error.message.contains("--top-k"));
    }

    #[test]
    fn parses_runtime_metadata_query_request() {
        let parsed = ParsedCommand::parse([
            "runtime-metadata",
            "--repo-id",
            "repo",
            "--revision-id",
            "rev",
            "--manifest-generation",
            "9",
            "--syntax",
            "sourcegraph",
            "--query-text",
            "lang:rust runtime",
            "--top-k",
            "3",
        ]);
        assert!(parsed.is_ok());
        let Ok(parsed) = parsed else {
            return;
        };
        let CliRequest::RuntimeMetadata(request) = parsed.request else {
            panic!("expected runtime-metadata payload");
        };
        assert_eq!(request.text_query.syntax, TextQuerySyntax::Sourcegraph);
        assert_eq!(request.text_query.query_text.as_str(), "lang:rust runtime");
        assert_eq!(request.text_query.top_k, 3);
        assert_eq!(
            request
                .text_query
                .generation
                .map(|pin| pin.manifest_generation.get()),
            Some(9)
        );
    }

    #[test]
    fn rejects_unknown_subcommand() {
        let parsed = ParsedCommand::parse(["nonsense"]);
        assert!(parsed.is_err());
        let Err(error) = parsed else {
            return;
        };
        assert_eq!(error.exit_code, EXIT_USAGE);
        assert!(error.message.contains("symbol"));
        assert!(error.message.contains("runtime-metadata"));
    }

    #[test]
    fn pretty_renderer_supports_symbol_response() {
        use quanta_index_contract::{
            RepoRelativePath, SymbolQueryResponse,
            lex::{SymbolKindCode, SymbolKindFamily},
        };
        let Ok(symbol_kind) = SymbolKindCode::new("function") else {
            return;
        };
        let response = SearchPlaneQueryIpcResponseEnvelope {
            request_id: 1,
            payload: SearchPlaneQueryIpcResponse::Symbol(SymbolQueryResponse {
                generation: GenerationPin::new(
                    RepoId::new("repo").expect("static fixture ID satisfies canonical policy"),
                    RevisionId::new("rev").expect("static fixture ID satisfies canonical policy"),
                    ManifestGeneration::new(7),
                ),
                results: vec![SymbolCandidate {
                    candidate_id: "sym-1".to_string(),
                    repo_id: RepoId::new("repo")
                        .expect("static fixture ID satisfies canonical policy"),
                    revision_id: RevisionId::new("rev")
                        .expect("static fixture ID satisfies canonical policy"),
                    manifest_generation: ManifestGeneration::new(7),
                    repo_relative_path: RepoRelativePath::new("src/lib.rs"),
                    start_line: 10,
                    end_line: 12,
                    score: 0.8,
                    snippet: "fn my_symbol() {}".to_string(),
                    symbol_kind,
                    symbol_kind_family: Some(SymbolKindFamily::Callable),
                }],
                window: QueryResultWindowV2::exact_probe(1),
                next_cursor: None,
            }),
        };
        let mut stdout = Vec::new();
        let rendered = render_response(OutputMode::Pretty, &response, &mut stdout);
        assert!(rendered.is_ok());
        let text = String::from_utf8(stdout);
        assert!(text.is_ok());
        if let Ok(text) = text {
            assert!(text.contains("kind: symbol"));
            assert!(text.contains("symbol_kind=function"));
            assert!(text.contains("symbol_kind_family=Callable"));
        }
    }

    #[test]
    fn pretty_renderer_supports_runtime_metadata_response() {
        use quanta_index_contract::RepoRelativePath;
        // One row returned of at least two: the continuation probe saw a
        // second match past the page.
        let probe_window = QueryResultWindowV2::pageable(
            1,
            quanta_index_contract::CandidateCountV1::AtLeast(2),
            true,
            Vec::new(),
        );
        assert!(probe_window.is_ok(), "{probe_window:?}");
        let Ok(probe_window) = probe_window else {
            return;
        };
        let response = SearchPlaneQueryIpcResponseEnvelope {
            request_id: 1,
            payload: SearchPlaneQueryIpcResponse::RuntimeMetadata(
                SearchPlaneRuntimeMetadataQueryResponse {
                    generation: GenerationPin::new(
                        RepoId::new("repo").expect("static fixture ID satisfies canonical policy"),
                        RevisionId::new("rev")
                            .expect("static fixture ID satisfies canonical policy"),
                        ManifestGeneration::new(7),
                    ),
                    results: vec![LexicalCandidate {
                        candidate_id: "rt-1".to_string(),
                        repo_id: RepoId::new("repo")
                            .expect("static fixture ID satisfies canonical policy"),
                        revision_id: RevisionId::new("rev")
                            .expect("static fixture ID satisfies canonical policy"),
                        manifest_generation: ManifestGeneration::new(7),
                        repo_relative_path: RepoRelativePath::new("src/runtime.rs"),
                        start_line: 1,
                        end_line: 4,
                        score: 0.3,
                        snippet: "runtime body".to_string(),
                        snippet_hit_offset: None,
                        highlights: Vec::new(),
                    }],
                    window: probe_window,
                    read_epoch: AuxEpochV1::new(4),
                    universe_epoch: AuxEpochV1::new(9),
                    examined: 2,
                    next_cursor: Some(
                        ContinuationTokenV2::new("runtime-token".to_string()).expect("token"),
                    ),
                },
            ),
        };
        let mut stdout = Vec::new();
        let rendered = render_response(OutputMode::Pretty, &response, &mut stdout);
        assert!(rendered.is_ok());
        let text = String::from_utf8(stdout);
        assert!(text.is_ok());
        if let Ok(text) = text {
            assert!(text.contains("kind: runtime-metadata"));
            assert!(text.contains("results: 1"));
            assert!(
                text.contains("epoch: 4"),
                "the read epoch is rendered: {text}"
            );
            assert!(
                text.contains("universe_epoch: 9"),
                "the universe epoch is rendered: {text}"
            );
            assert!(
                text.contains("order: candidate_id matched: >=2 examined: 2 has_more: true"),
                "the probe window is rendered: {text}"
            );
            assert!(
                text.contains("next_cursor: \"runtime-token\""),
                "the continuation is rendered: {text}"
            );
        }
    }

    #[test]
    fn pretty_renderer_supports_sourcegraph_text_response() {
        let response = SearchPlaneQueryIpcResponseEnvelope {
            request_id: 1,
            payload: SearchPlaneQueryIpcResponse::Text(TextQueryResponse {
                generation: GenerationPin::new(
                    RepoId::new("repo").expect("static fixture ID satisfies canonical policy"),
                    RevisionId::new("rev").expect("static fixture ID satisfies canonical policy"),
                    ManifestGeneration::new(7),
                ),
                results: vec![LexicalCandidate {
                    candidate_id: "cand-1".to_string(),
                    repo_id: RepoId::new("repo")
                        .expect("static fixture ID satisfies canonical policy"),
                    revision_id: RevisionId::new("rev")
                        .expect("static fixture ID satisfies canonical policy"),
                    manifest_generation: ManifestGeneration::new(7),
                    repo_relative_path: quanta_index_contract::RepoRelativePath::new("src/lib.rs"),
                    start_line: 1,
                    end_line: 3,
                    score: 0.5,
                    snippet: "fn sample() {}".to_string(),
                    snippet_hit_offset: None,
                    highlights: Vec::new(),
                }],
                window: QueryResultWindowV2::exact_probe(1),
                file_owner_rows: Some(vec![quanta_index_contract::FileOwnerProjectionRow {
                    candidate_id: "cand-1".to_string(),
                    repo_id: RepoId::new("repo")
                        .expect("static fixture ID satisfies canonical policy"),
                    revision_id: RevisionId::new("rev")
                        .expect("static fixture ID satisfies canonical policy"),
                    manifest_generation: ManifestGeneration::new(7),
                    repo_relative_path: quanta_index_contract::RepoRelativePath::new("src/lib.rs"),
                    owners: vec!["@alice".to_string(), "@acme/platform".to_string()],
                }]),
                next_cursor: None,
            }),
        };
        let mut stdout = Vec::new();
        let rendered = render_response(OutputMode::Pretty, &response, &mut stdout);
        assert!(rendered.is_ok());
        let text = String::from_utf8(stdout);
        assert!(text.is_ok());
        if let Ok(text) = text {
            assert!(text.contains("kind: lexical"));
            assert!(text.contains("results: 1"));
            assert!(text.contains("file_owner_rows: 1"));
            assert!(text.contains("owners=@alice,@acme/platform"));
        }
    }

    #[test]
    fn parses_history_query_request() {
        let parsed = ParsedCommand::parse([
            "history",
            "--repo-id",
            "repo",
            "--revision-id",
            "rev",
            "--manifest-generation",
            "7",
            "--syntax",
            "native",
            "--query-text",
            "feat: add x",
            "--top-k",
            "9",
            "--order",
            "relevance",
        ]);
        assert!(parsed.is_ok());
        let Ok(parsed) = parsed else {
            return;
        };
        let CliRequest::History(request) = parsed.request else {
            panic!("expected history payload");
        };
        assert_eq!(request.text_query.syntax, TextQuerySyntax::Native);
        assert_eq!(request.text_query.query_text.as_str(), "feat: add x");
        assert_eq!(request.text_query.top_k, 9);
        assert_eq!(request.order, HistoryOrderV1::Relevance);
        assert!(request.cursor.is_none());
        assert_eq!(
            request
                .text_query
                .generation
                .map(|pin| pin.manifest_generation.get()),
            Some(7)
        );
    }

    /// The history order is required and closed: no flag or an unknown
    /// value is a usage error, never a default.
    #[test]
    fn history_requires_a_known_order() {
        let base = [
            "history",
            "--repo-id",
            "repo",
            "--revision-id",
            "rev",
            "--manifest-generation",
            "7",
            "--syntax",
            "native",
            "--query-text",
            "feat: add x",
            "--top-k",
            "9",
        ];
        let missing = ParsedCommand::parse(base);
        let Err(error) = missing else {
            panic!("history without --order must be a usage error");
        };
        assert_eq!(error.exit_code, EXIT_USAGE);
        assert!(error.message.contains("--order"), "{}", error.message);

        let mut unknown: Vec<&str> = base.to_vec();
        unknown.extend(["--order", "newest"]);
        let Err(error) = ParsedCommand::parse(unknown) else {
            panic!("an unknown order must be a usage error");
        };
        assert_eq!(error.exit_code, EXIT_USAGE);
        assert!(error.message.contains("newest"), "{}", error.message);

        let mut recency: Vec<&str> = base.to_vec();
        recency.extend(["--order", "recency"]);
        let parsed = ParsedCommand::parse(recency).expect("recency parses");
        let CliRequest::History(request) = parsed.request else {
            panic!("expected history payload");
        };
        assert_eq!(request.order, HistoryOrderV1::Recency);
    }

    #[test]
    fn parses_structural_query_request() {
        let parsed = ParsedCommand::parse([
            "structural",
            "--repo-id",
            "repo",
            "--revision-id",
            "rev",
            "--manifest-generation",
            "7",
            "--syntax",
            "sourcegraph",
            "--query-text",
            "lang:rust fn $NAME(...) {...}",
            "--top-k",
            "4",
        ]);
        assert!(parsed.is_ok());
        let Ok(parsed) = parsed else {
            return;
        };
        let CliRequest::Structural(request) = parsed.request else {
            panic!("expected structural payload");
        };
        assert_eq!(request.text_query.syntax, TextQuerySyntax::Sourcegraph);
        assert_eq!(
            request.text_query.query_text.as_str(),
            "lang:rust fn $NAME(...) {...}"
        );
        assert_eq!(request.text_query.top_k, 4);
    }

    #[test]
    fn rejects_history_missing_top_k() {
        let parsed = ParsedCommand::parse([
            "history",
            "--repo-id",
            "repo",
            "--revision-id",
            "rev",
            "--manifest-generation",
            "7",
            "--syntax",
            "native",
            "--query-text",
            "feat",
        ]);
        assert!(parsed.is_err());
        let Err(error) = parsed else {
            return;
        };
        assert_eq!(error.exit_code, EXIT_USAGE);
        assert!(error.message.contains("--top-k"));
    }

    #[test]
    fn rejects_structural_missing_query_text() {
        let parsed = ParsedCommand::parse([
            "structural",
            "--repo-id",
            "repo",
            "--revision-id",
            "rev",
            "--manifest-generation",
            "7",
            "--syntax",
            "native",
            "--top-k",
            "4",
        ]);
        assert!(parsed.is_err());
        let Err(error) = parsed else {
            return;
        };
        assert_eq!(error.exit_code, EXIT_USAGE);
        assert!(error.message.contains("--query-text"));
    }

    #[test]
    fn pretty_renderer_supports_history_response() {
        let response = SearchPlaneQueryIpcResponseEnvelope {
            request_id: 1,
            payload: SearchPlaneQueryIpcResponse::History(SearchPlaneHistoryQueryResponse {
                generation: GenerationPin::new(
                    RepoId::new("repo").expect("static fixture ID satisfies canonical policy"),
                    RevisionId::new("rev").expect("static fixture ID satisfies canonical policy"),
                    ManifestGeneration::new(7),
                ),
                order: HistoryOrderV1::Recency,
                commits: vec![quanta_index_contract::CommitCandidate {
                    sha: quanta_index_contract::lex::CommitSha::ZERO,
                    parent_ids: Vec::new(),
                    committed_at_unix_s: 1_700_000_000,
                    author: "alice".to_string(),
                    committer: "alice".to_string(),
                    message: "fix: thing\nbody line".to_string(),
                    is_merge: false,
                    tags: vec!["v1.0".to_string()],
                    score: None,
                }],
                diffs: Vec::new(),
                window: QueryResultWindowV2::pageable(
                    1,
                    quanta_index_contract::CandidateCountV1::AtLeast(3),
                    true,
                    Vec::new(),
                )
                .expect("a page of one out of three"),
                read_epoch: AuxEpochV1::new(12),
                examined: 9,
                next_cursor: Some(
                    ContinuationTokenV2::new("history-recency-token".to_string()).expect("token"),
                ),
            }),
        };
        let mut stdout = Vec::new();
        let rendered = render_response(OutputMode::Pretty, &response, &mut stdout);
        assert!(rendered.is_ok());
        let text = String::from_utf8(stdout).expect("utf-8");
        assert!(
            text.contains("order: recency matched: >=3 examined: 9 has_more: true"),
            "{text}"
        );
        assert!(
            text.contains("next_cursor: \"history-recency-token\""),
            "{text}"
        );
        assert!(
            !text.contains("score="),
            "a recency page renders no score: {text}"
        );
        assert!(
            text.contains("epoch: 12"),
            "the read epoch is rendered: {text}"
        );
        assert!(text.contains("kind: history"));
        assert!(text.contains("commits: 1 diffs: 0"));
        assert!(text.contains("author=alice"));
    }

    /// A relevance page says so and prints each row's score and the
    /// cursor's score.
    #[test]
    fn pretty_renderer_prints_relevance_scores() {
        let score = quanta_index_contract::HistoryScoreV1::try_new(1.5).expect("finite");
        let response = SearchPlaneQueryIpcResponseEnvelope {
            request_id: 1,
            payload: SearchPlaneQueryIpcResponse::History(SearchPlaneHistoryQueryResponse {
                generation: GenerationPin::new(
                    RepoId::new("repo").expect("static fixture ID satisfies canonical policy"),
                    RevisionId::new("rev").expect("static fixture ID satisfies canonical policy"),
                    ManifestGeneration::new(7),
                ),
                order: HistoryOrderV1::Relevance,
                commits: Vec::new(),
                diffs: vec![quanta_index_contract::DiffCandidate {
                    repo_relative_path: "src/lib.rs".to_string(),
                    hunk_header: "@@ -1 +1 @@".to_string(),
                    side: quanta_index_contract::DiffHunkSide::After,
                    line_start: 1,
                    line_end: 2,
                    snippet: "needle".to_string(),
                    score: Some(score),
                }],
                window: QueryResultWindowV2::pageable(
                    1,
                    quanta_index_contract::CandidateCountV1::AtLeast(2),
                    true,
                    Vec::new(),
                )
                .expect("a page of one out of two"),
                read_epoch: AuxEpochV1::new(3),
                examined: 4,
                next_cursor: Some(
                    ContinuationTokenV2::new("history-relevance-token".to_string()).expect("token"),
                ),
            }),
        };
        let mut stdout = Vec::new();
        let rendered = render_response(OutputMode::Pretty, &response, &mut stdout);
        assert!(rendered.is_ok());
        let text = String::from_utf8(stdout).expect("utf-8");
        assert!(
            text.contains("order: relevance matched: >=2 examined: 4 has_more: true"),
            "{text}"
        );
        assert!(
            text.contains("1. score=1.5 path=src/lib.rs"),
            "each row carries its score: {text}"
        );
        assert!(
            text.contains("next_cursor: \"history-relevance-token\""),
            "the opaque cursor is rendered: {text}"
        );
    }

    #[test]
    fn parses_readiness_request() {
        let parsed =
            ParsedCommand::parse(["readiness", "--repo-id", "repo-1", "--revision-id", "rev-1"]);
        assert!(parsed.is_ok());
        let Ok(parsed) = parsed else {
            return;
        };
        assert_eq!(parsed.kind, CommandKind::Readiness);
        let CliRequest::Readiness {
            repo_id,
            revision_id,
        } = parsed.request
        else {
            panic!("expected readiness payload");
        };
        assert_eq!(repo_id.as_str(), "repo-1");
        assert_eq!(revision_id.as_str(), "rev-1");
    }

    #[test]
    fn rejects_readiness_missing_repo_id() {
        let parsed = ParsedCommand::parse(["readiness", "--revision-id", "rev-1"]);
        assert!(parsed.is_err());
        let Err(error) = parsed else {
            return;
        };
        assert_eq!(error.exit_code, EXIT_USAGE);
        assert!(error.message.contains("--repo-id"));
    }

    #[test]
    fn rejects_readiness_missing_revision_id() {
        let parsed = ParsedCommand::parse(["readiness", "--repo-id", "repo-1"]);
        assert!(parsed.is_err());
        let Err(error) = parsed else {
            return;
        };
        assert_eq!(error.exit_code, EXIT_USAGE);
        assert!(error.message.contains("--revision-id"));
    }

    fn metrics_fixture() -> MetricsSnapshotV1 {
        use quanta_index_contract::{
            MetricBucketV1, MetricCounterV1, MetricGaugeV1, MetricsDiagnosticsV1,
        };
        MetricsSnapshotV1 {
            counters: vec![
                MetricCounterV1 {
                    name: "ipc_query_requests_dispatched_total".to_string(),
                    value: 13,
                },
                MetricCounterV1 {
                    name: "lq_route_lexical_served_total".to_string(),
                    value: 12,
                },
            ],
            gauges: vec![MetricGaugeV1 {
                name: "ipc_query_connections_live".to_string(),
                value: 1.0,
            }],
            histograms: vec![MetricHistogramV1 {
                name: "lq_route_lexical_latency_ms".to_string(),
                count: 3,
                sum: 8.5,
                min: 0.5,
                max: 7.0,
                buckets: vec![
                    MetricBucketV1 { le: 1.0, count: 2 },
                    MetricBucketV1 { le: 2.5, count: 2 },
                    MetricBucketV1 { le: 10.0, count: 3 },
                ],
            }],
            diagnostics: MetricsDiagnosticsV1 {
                samples_recorded: 40,
                samples_dropped: 0,
                errors_recorded: 2,
                errors_dropped: 0,
            },
        }
    }

    /// QI-BB-015: the pretty rendering is line-oriented and complete.
    #[test]
    fn render_metrics_pretty_lists_every_series_and_the_diagnostics() {
        let rendered = render_metrics(&metrics_fixture(), OutputMode::Pretty);
        assert!(rendered.is_ok(), "{rendered:?}");
        let Ok(text) = rendered else {
            return;
        };
        assert_eq!(
            text,
            "kind: metrics\n\
             counters: 2\n\
             \x20 ipc_query_requests_dispatched_total 13\n\
             \x20 lq_route_lexical_served_total 12\n\
             gauges: 1\n\
             \x20 ipc_query_connections_live 1\n\
             histograms: 1\n\
             \x20 lq_route_lexical_latency_ms count=3 sum=8.5 min=0.5 max=7\n\
             \x20   le=1 2\n\
             \x20   le=2.5 2\n\
             \x20   le=10 3\n\
             \x20   le=+Inf 3\n\
             diagnostics: samples_recorded=40 samples_dropped=0 errors_recorded=2 errors_dropped=0\n"
        );
    }

    /// QI-BB-015: the Prometheus exposition is exactly what a scraper reads.
    ///
    /// Typed families, cumulative `_bucket` series ending in a `+Inf` bucket
    /// spelled from `count`, and the diagnostics as their own counters.
    #[test]
    fn render_metrics_prometheus_emits_the_text_exposition_format() {
        let rendered = render_metrics(&metrics_fixture(), OutputMode::Prometheus);
        assert!(rendered.is_ok(), "{rendered:?}");
        let Ok(text) = rendered else {
            return;
        };
        assert_eq!(
            text,
            "# TYPE ipc_query_requests_dispatched_total counter\n\
             ipc_query_requests_dispatched_total 13\n\
             # TYPE lq_route_lexical_served_total counter\n\
             lq_route_lexical_served_total 12\n\
             # TYPE ipc_query_connections_live gauge\n\
             ipc_query_connections_live 1\n\
             # TYPE lq_route_lexical_latency_ms histogram\n\
             lq_route_lexical_latency_ms_bucket{le=\"1\"} 2\n\
             lq_route_lexical_latency_ms_bucket{le=\"2.5\"} 2\n\
             lq_route_lexical_latency_ms_bucket{le=\"10\"} 3\n\
             lq_route_lexical_latency_ms_bucket{le=\"+Inf\"} 3\n\
             lq_route_lexical_latency_ms_sum 8.5\n\
             lq_route_lexical_latency_ms_count 3\n\
             # TYPE searchd_obs_samples_recorded_total counter\n\
             searchd_obs_samples_recorded_total 40\n\
             # TYPE searchd_obs_samples_dropped_total counter\n\
             searchd_obs_samples_dropped_total 0\n\
             # TYPE searchd_obs_errors_recorded_total counter\n\
             searchd_obs_errors_recorded_total 2\n\
             # TYPE searchd_obs_errors_dropped_total counter\n\
             searchd_obs_errors_dropped_total 0\n"
        );
    }

    /// QI-BB-015: `json` is the wire shape, decodable back into the snapshot.
    #[test]
    fn render_metrics_json_round_trips_the_snapshot() {
        let fixture = metrics_fixture();
        let rendered = render_metrics(&fixture, OutputMode::Json);
        assert!(rendered.is_ok(), "{rendered:?}");
        let Ok(text) = rendered else {
            return;
        };
        let decoded: Result<MetricsSnapshotV1, _> = serde_json::from_str(&text);
        assert!(decoded.is_ok(), "{decoded:?}");
        let Ok(decoded) = decoded else {
            return;
        };
        assert_eq!(decoded, fixture);
    }

    /// QI-BB-015: `metrics` takes only the global flags, and `prometheus`
    /// output belongs to it alone.
    #[test]
    fn metrics_parses_alone_and_prometheus_output_is_refused_elsewhere() {
        let parsed = ParsedCommand::parse(["metrics", "--output", "prometheus"]);
        assert!(parsed.is_ok(), "{parsed:?}");
        let Ok(parsed) = parsed else {
            return;
        };
        assert_eq!(parsed.kind, CommandKind::Metrics);
        assert_eq!(parsed.output, OutputMode::Prometheus);
        assert_eq!(parsed.request, CliRequest::Metrics);

        let with_flag = ParsedCommand::parse(["metrics", "--repo-id", "repo"]);
        assert!(with_flag.is_err(), "{with_flag:?}");
        let Err(error) = with_flag else {
            return;
        };
        assert_eq!(error.exit_code, EXIT_USAGE);
        assert!(error.message.contains("unknown metrics flag `--repo-id`"));

        let readiness = ParsedCommand::parse([
            "readiness",
            "--repo-id",
            "repo",
            "--revision-id",
            "rev",
            "--output",
            "prometheus",
        ]);
        assert!(readiness.is_err(), "{readiness:?}");
        let Err(error) = readiness else {
            return;
        };
        assert_eq!(error.exit_code, EXIT_USAGE);
        assert!(
            error
                .message
                .contains("renders only `metrics`, not `readiness`"),
            "{}",
            error.message
        );
    }

    /// QI-BB-026: `quarantine list` takes only the global flags; a discard
    /// names one entry exactly and every incomplete or mixed form is a
    /// usage error before anything reaches the socket.
    #[test]
    fn quarantine_parses_list_and_exact_discard_targets_only() {
        let parsed = ParsedCommand::parse(["quarantine", "list", "--output", "json"]);
        assert!(parsed.is_ok(), "{parsed:?}");
        let Ok(parsed) = parsed else {
            return;
        };
        assert_eq!(parsed.kind, CommandKind::Quarantine);
        assert_eq!(parsed.output, OutputMode::Json);
        assert_eq!(parsed.request, CliRequest::QuarantineList);

        let generation = ParsedCommand::parse([
            "quarantine",
            "discard",
            "--track",
            "semantic",
            "--path",
            "/state/indexes/semantic/repo/rev/g3",
            "--reason",
            "GENERATION_QUARANTINE_SCOPE_MISMATCH",
            "--detail",
            "identity names repo other",
        ]);
        assert!(generation.is_ok(), "{generation:?}");
        let Ok(generation) = generation else {
            return;
        };
        assert_eq!(
            generation.request,
            CliRequest::QuarantineDiscard(QuarantineTargetV1::Generation(
                QuarantinedGenerationEntryV1 {
                    track: SearchPlaneTrackKind::Semantic,
                    path: "/state/indexes/semantic/repo/rev/g3".to_string(),
                    reason: "GENERATION_QUARANTINE_SCOPE_MISMATCH".to_string(),
                    detail: "identity names repo other".to_string(),
                }
            ))
        );
        let without_detail = ParsedCommand::parse([
            "quarantine",
            "discard",
            "--track",
            "lexical",
            "--path",
            "/state/indexes/lexical/repo-legacy",
            "--reason",
            "GENERATION_QUARANTINE_NON_CANONICAL_LAYOUT",
        ]);
        assert!(without_detail.is_ok(), "{without_detail:?}");
        let Ok(without_detail) = without_detail else {
            return;
        };
        assert_eq!(
            without_detail.request,
            CliRequest::QuarantineDiscard(QuarantineTargetV1::Generation(
                QuarantinedGenerationEntryV1 {
                    track: SearchPlaneTrackKind::Lexical,
                    path: "/state/indexes/lexical/repo-legacy".to_string(),
                    reason: "GENERATION_QUARANTINE_NON_CANONICAL_LAYOUT".to_string(),
                    detail: String::new(),
                }
            ))
        );
        let repo_map = ParsedCommand::parse([
            "quarantine",
            "discard",
            "--repomap-file",
            "stale--marker.json",
            "--reason",
            "snapshot does not decode",
        ]);
        assert!(repo_map.is_ok(), "{repo_map:?}");
        let Ok(repo_map) = repo_map else {
            return;
        };
        assert_eq!(
            repo_map.request,
            CliRequest::QuarantineDiscard(QuarantineTargetV1::RepoMapFile(
                QuarantinedRepoMapFileEntryV1 {
                    file_name: "stale--marker.json".to_string(),
                    reason: "snapshot does not decode".to_string(),
                }
            ))
        );

        for (args, needle) in [
            (vec!["quarantine"], "requires `list` or `discard`"),
            (
                vec!["quarantine", "purge"],
                "unknown quarantine verb `purge`",
            ),
            (
                vec!["quarantine", "list", "--all"],
                "unknown quarantine list flag `--all`",
            ),
            (vec!["quarantine", "discard"], "needs --track and --path"),
            (
                vec![
                    "quarantine",
                    "discard",
                    "--track",
                    "lexical",
                    "--path",
                    "/p",
                ],
                "--path requires --reason",
            ),
            (
                vec![
                    "quarantine",
                    "discard",
                    "--track",
                    "lexical",
                    "--reason",
                    "R",
                ],
                "needs --track and --path",
            ),
            (
                vec!["quarantine", "discard", "--path", "/p", "--reason", "R"],
                "needs --track and --path",
            ),
            (
                vec![
                    "quarantine",
                    "discard",
                    "--track",
                    "repomap",
                    "--path",
                    "/p",
                    "--reason",
                    "R",
                ],
                "unsupported quarantine track `repomap`",
            ),
            (
                vec!["quarantine", "discard", "--repomap-file", "x.json"],
                "--repomap-file requires --reason",
            ),
            (
                vec![
                    "quarantine",
                    "discard",
                    "--repomap-file",
                    "x.json",
                    "--track",
                    "lexical",
                    "--reason",
                    "R",
                ],
                "cannot be combined",
            ),
            (
                vec!["quarantine", "discard", "--force"],
                "unknown quarantine discard flag `--force`",
            ),
            (
                vec!["quarantine", "list", "--output", "prometheus"],
                "renders only `metrics`, not `quarantine`",
            ),
        ] {
            let parsed = ParsedCommand::parse(args.clone());
            let Err(error) = parsed else {
                panic!("{args:?} must be a usage error, parsed {parsed:?}");
            };
            assert_eq!(error.exit_code, EXIT_USAGE, "{args:?}: {}", error.message);
            assert!(
                error.message.contains(needle),
                "{args:?}: {} lacks {needle:?}",
                error.message
            );
        }
    }

    /// QI-BB-026: the pretty inventory prints each entry as the discard
    /// flags that name it, the JSON form is the wire DTO, and the ack
    /// renders the target and outcome the daemon returned.
    #[test]
    fn render_quarantine_prints_discardable_lines_and_the_ack() {
        let inventory = QuarantineInventoryV1 {
            lexical: vec![QuarantinedGenerationEntryV1 {
                track: SearchPlaneTrackKind::Lexical,
                path: "/state/indexes/lexical/repo/rev/g9".to_string(),
                reason: "GENERATION_QUARANTINE_IDENTITY_UNREADABLE".to_string(),
                detail: "identity file does not decode".to_string(),
            }],
            semantic: Vec::new(),
            repo_map: vec![QuarantinedRepoMapFileEntryV1 {
                file_name: "stale--marker.json".to_string(),
                reason: "snapshot does not decode".to_string(),
            }],
        };
        let pretty = render_quarantine_inventory(&inventory, OutputMode::Pretty);
        let Ok(pretty) = pretty else {
            panic!("pretty inventory renders: {pretty:?}");
        };
        let expected = [
            "kind: quarantine",
            "lexical: 1",
            r#"  --track lexical --path /state/indexes/lexical/repo/rev/g9 --reason GENERATION_QUARANTINE_IDENTITY_UNREADABLE --detail "identity file does not decode""#,
            "semantic: 0",
            "repo_map: 1",
            r#"  --repomap-file stale--marker.json --reason "snapshot does not decode""#,
            "",
        ]
        .join("\n");
        assert_eq!(pretty, expected);
        let json = render_quarantine_inventory(&inventory, OutputMode::Json);
        let Ok(json) = json else {
            panic!("json inventory renders: {json:?}");
        };
        let Ok(decoded) = serde_json::from_str::<QuarantineInventoryV1>(&json) else {
            panic!("json inventory decodes back: {json}");
        };
        assert_eq!(decoded, inventory);
        let prometheus = render_quarantine_inventory(&inventory, OutputMode::Prometheus);
        let Err(error) = prometheus else {
            panic!("prometheus output is metrics-only: {prometheus:?}");
        };
        assert_eq!(error.exit_code, EXIT_USAGE);

        let ack = QuarantineDiscardAck {
            target: QuarantineTargetV1::RepoMapFile(QuarantinedRepoMapFileEntryV1 {
                file_name: "stale--marker.json".to_string(),
                reason: "snapshot does not decode".to_string(),
            }),
            outcome: QuarantineDiscardOutcomeDtoV1::Discarded { bytes: 15 },
        };
        let pretty = render_quarantine_discard(&ack, OutputMode::Pretty);
        let Ok(pretty) = pretty else {
            panic!("pretty ack renders: {pretty:?}");
        };
        assert_eq!(
            pretty,
            "kind: quarantine-discard\ntarget: repo_map stale--marker.json\noutcome: discarded bytes=15\n"
        );
        let absent = QuarantineDiscardAck {
            target: QuarantineTargetV1::Generation(QuarantinedGenerationEntryV1 {
                track: SearchPlaneTrackKind::Semantic,
                path: "/state/indexes/semantic/repo/rev/g1".to_string(),
                reason: "GENERATION_QUARANTINE_SCOPE_MISMATCH".to_string(),
                detail: String::new(),
            }),
            outcome: QuarantineDiscardOutcomeDtoV1::Absent,
        };
        let pretty = render_quarantine_discard(&absent, OutputMode::Pretty);
        let Ok(pretty) = pretty else {
            panic!("pretty ack renders: {pretty:?}");
        };
        assert_eq!(
            pretty,
            "kind: quarantine-discard\ntarget: semantic /state/indexes/semantic/repo/rev/g1\noutcome: absent\n"
        );
        let json = render_quarantine_discard(&absent, OutputMode::Json);
        let Ok(json) = json else {
            panic!("json ack renders: {json:?}");
        };
        let Ok(decoded) = serde_json::from_str::<QuarantineDiscardAck>(&json) else {
            panic!("json ack decodes back: {json}");
        };
        assert_eq!(decoded, absent);
    }

    #[test]
    fn render_readiness_json_emits_report_shape() {
        use quanta_index_contract::ipc::{
            GenerationStatusReport, SearchPlaneTrackKind, TrackReadinessRecord,
        };
        let report = GenerationStatusReport {
            repo_id: RepoId::new("repo-1").expect("static fixture ID satisfies canonical policy"),
            revision_id: RevisionId::new("rev-1")
                .expect("static fixture ID satisfies canonical policy"),
            semantic_content: None,
            tracks: vec![TrackReadinessRecord {
                track: SearchPlaneTrackKind::Lexical,
                manifest_generation: ManifestGeneration::new(11),
                manifest_digest: "digest-11".to_string(),
            }],
        };
        let rendered = render_readiness(&report, OutputMode::Json);
        assert!(rendered.is_ok());
        let Ok(text) = rendered else {
            return;
        };
        let parsed = serde_json::from_str::<serde_json::Value>(&text);
        assert!(parsed.is_ok(), "json output must parse: {text}");
        assert!(text.contains("\"repo_id\": \"repo-1\""));
        assert!(text.contains("\"revision_id\": \"rev-1\""));
        assert!(text.contains("\"track\": \"Lexical\""));
        assert!(text.contains("\"manifest_digest\": \"digest-11\""));
        assert!(text.ends_with('\n'));
    }

    #[test]
    fn render_readiness_pretty_marks_empty_tracks() {
        use quanta_index_contract::ipc::GenerationStatusReport;
        let report = GenerationStatusReport {
            repo_id: RepoId::new("repo-1").expect("static fixture ID satisfies canonical policy"),
            revision_id: RevisionId::new("rev-1")
                .expect("static fixture ID satisfies canonical policy"),
            semantic_content: None,
            tracks: vec![],
        };
        let rendered = render_readiness(&report, OutputMode::Pretty);
        assert!(rendered.is_ok());
        let Ok(text) = rendered else {
            return;
        };
        assert!(text.contains("kind: readiness"));
        assert!(text.contains("tracks: 0 (none activated)"));
    }

    #[test]
    fn render_readiness_pretty_lists_tracks() {
        use quanta_index_contract::ipc::{
            GenerationStatusReport, SearchPlaneTrackKind, TrackReadinessRecord,
        };
        let report = GenerationStatusReport {
            repo_id: RepoId::new("repo-1").expect("static fixture ID satisfies canonical policy"),
            revision_id: RevisionId::new("rev-1")
                .expect("static fixture ID satisfies canonical policy"),
            semantic_content: None,
            tracks: vec![
                TrackReadinessRecord {
                    track: SearchPlaneTrackKind::Lexical,
                    manifest_generation: ManifestGeneration::new(11),
                    manifest_digest: "lex".to_string(),
                },
                TrackReadinessRecord {
                    track: SearchPlaneTrackKind::Semantic,
                    manifest_generation: ManifestGeneration::new(12),
                    manifest_digest: "sem".to_string(),
                },
            ],
        };
        let rendered = render_readiness(&report, OutputMode::Pretty);
        assert!(rendered.is_ok());
        let Ok(text) = rendered else {
            return;
        };
        assert!(text.contains("tracks: 2"));
        assert!(text.contains("1. track=Lexical manifest_generation=11 manifest_digest=lex"));
        assert!(text.contains("2. track=Semantic manifest_generation=12 manifest_digest=sem"));
    }

    #[test]
    fn pretty_renderer_supports_structural_response() {
        let response = SearchPlaneQueryIpcResponseEnvelope {
            request_id: 1,
            payload: SearchPlaneQueryIpcResponse::Structural(SearchPlaneStructuralQueryResponse {
                generation: GenerationPin::new(
                    RepoId::new("repo").expect("static fixture ID satisfies canonical policy"),
                    RevisionId::new("rev").expect("static fixture ID satisfies canonical policy"),
                    ManifestGeneration::new(7),
                ),
                results: vec![quanta_index_contract::StructuralCandidate {
                    candidate_id: "struct-1".to_string(),
                    bindings: vec![quanta_index_contract::StructuralBinding {
                        metavariable: "$NAME".to_string(),
                        start_byte: 10,
                        end_byte: 14,
                        start_line: 2,
                        end_line: 2,
                    }],
                }],
                window: QueryResultWindowV2::exact_probe(1),
                read_epoch: AuxEpochV1::new(3),
                examined: 1,
                next_cursor: None,
            }),
        };
        let mut stdout = Vec::new();
        let rendered = render_response(OutputMode::Pretty, &response, &mut stdout);
        assert!(rendered.is_ok());
        let text = String::from_utf8(stdout);
        assert!(text.is_ok());
        if let Ok(text) = text {
            assert!(text.contains("kind: structural"));
            assert!(
                text.contains("epoch: 3"),
                "the read epoch is rendered: {text}"
            );
            assert!(text.contains("results: 1"));
            assert!(text.contains("candidate_id=struct-1"));
            assert!(text.contains("$NAME: bytes=10-14"));
            assert!(
                text.contains("order: candidate_id matched: 1 examined: 1 has_more: false"),
                "the exact window is rendered: {text}"
            );
            assert!(
                !text.contains("next_cursor:"),
                "a final page prints no continuation: {text}"
            );
        }
    }

    fn sample_candidate(id: &str, score: f32) -> LexicalCandidate {
        LexicalCandidate {
            candidate_id: id.to_string(),
            repo_id: RepoId::new("repo").expect("static fixture ID satisfies canonical policy"),
            revision_id: RevisionId::new("rev")
                .expect("static fixture ID satisfies canonical policy"),
            manifest_generation: ManifestGeneration::new(7),
            repo_relative_path: quanta_index_contract::RepoRelativePath::new("src/lib.rs"),
            start_line: 1,
            end_line: 3,
            score,
            snippet: "fn sample() {}".to_string(),
            snippet_hit_offset: None,
            highlights: Vec::new(),
        }
    }

    /// A hybrid row both lanes saw: the lexical lane's row at lexical rank
    /// 1 and dense rank 2, with the RRF score of those ranks.
    fn sample_hybrid_candidate() -> HybridCandidateV1 {
        HybridCandidateV1 {
            candidate: sample_candidate("cand-1", 2.5),
            fused_score: 1.0 / 61.0 + 1.0 / 62.0,
            contributions: vec![
                quanta_index_contract::HybridLaneContributionV1 {
                    lane: quanta_index_contract::HybridLaneV1::Lexical,
                    rank: 1,
                    raw_score: 2.5,
                },
                quanta_index_contract::HybridLaneContributionV1 {
                    lane: quanta_index_contract::HybridLaneV1::Dense,
                    rank: 2,
                    raw_score: 0.75,
                },
            ],
        }
    }

    // QI-BB-022: one line per fused row names the RRF score and each lane's
    // rank and raw score; the JSON path is the wire DTO itself.
    #[test]
    fn pretty_renderer_supports_hybrid_response_with_lane_provenance() {
        let response = SearchPlaneQueryIpcResponseEnvelope {
            request_id: 1,
            payload: SearchPlaneQueryIpcResponse::Hybrid(HybridQueryResponse {
                generation: GenerationPin::new(
                    RepoId::new("repo").expect("static fixture ID satisfies canonical policy"),
                    RevisionId::new("rev").expect("static fixture ID satisfies canonical policy"),
                    ManifestGeneration::new(7),
                ),
                results: vec![
                    sample_hybrid_candidate(),
                    HybridCandidateV1 {
                        candidate: sample_candidate("cand-2", 0.5),
                        fused_score: 1.0 / 61.0,
                        contributions: vec![quanta_index_contract::HybridLaneContributionV1 {
                            lane: quanta_index_contract::HybridLaneV1::Dense,
                            rank: 1,
                            raw_score: 0.5,
                        }],
                    },
                ],
                window: QueryResultWindowV2::exact_probe(2),
                explanation: SearchExplanation {
                    planner_trace: Vec::new(),
                    engines_touched: vec![EngineTouched::Lexical, EngineTouched::Semantic],
                    engines_executed: vec![EngineTouched::Lexical, EngineTouched::Semantic],
                    request_id: 0,
                    early_stop_reason: None,
                    contributions: Vec::new(),
                    ranker_weights_hash: [0u8; 32],
                    strategy: "rrf".to_string(),
                    summary: "two lanes".to_string(),
                },
            }),
        };
        let mut stdout = Vec::new();
        let rendered = render_response(OutputMode::Pretty, &response, &mut stdout);
        assert!(rendered.is_ok());
        let text = String::from_utf8(stdout);
        assert!(text.is_ok());
        if let Ok(text) = text {
            assert!(text.contains("kind: hybrid"), "{text}");
            assert!(text.contains("results: 2"), "{text}");
            assert!(
                text.contains(&format!(
                    "1. candidate_id=cand-1 path=src/lib.rs lines=1-3 score=2.5 fused={} lanes=lexical#1(2.5) dense#2(0.75)",
                    1.0_f64 / 61.0 + 1.0 / 62.0
                )),
                "{text}"
            );
            assert!(
                text.contains(&format!(
                    "2. candidate_id=cand-2 path=src/lib.rs lines=1-3 score=0.5 fused={} lanes=dense#1(0.5)",
                    1.0_f64 / 61.0
                )),
                "{text}"
            );
            assert!(text.contains("strategy: rrf"), "{text}");
        }
        let mut json = Vec::new();
        let rendered = render_response(OutputMode::Json, &response, &mut json);
        assert!(rendered.is_ok());
        let decoded: Result<SearchPlaneQueryIpcResponseEnvelope, _> = serde_json::from_slice(&json);
        assert!(
            decoded.is_ok(),
            "the JSON output is the wire DTO: {decoded:?}"
        );
        if let Ok(decoded) = decoded {
            assert_eq!(decoded, response);
        }
    }

    // A hybrid row is explained from the JSON the hybrid route emitted for
    // it, and only under its query.
    #[test]
    fn parses_explain_with_a_hybrid_candidate_json_under_its_query() {
        let dir = tempfile::tempdir();
        assert!(dir.is_ok());
        let Ok(dir) = dir else {
            return;
        };
        let path = dir.path().join("hybrid-candidate.json");
        let written = serde_json::to_vec_pretty(&sample_hybrid_candidate())
            .map_err(|err| err.to_string())
            .and_then(|bytes| fs::write(&path, bytes).map_err(|err| err.to_string()));
        assert!(written.is_ok(), "{written:?}");
        let path = path.to_string_lossy().into_owned();
        let parsed = ParsedCommand::parse([
            "explain",
            "--repo-id",
            "repo",
            "--revision-id",
            "rev",
            "--manifest-generation",
            "7",
            "--hybrid-candidate-json",
            path.as_str(),
            "--syntax",
            "native",
            "--query-text",
            "needle",
            "--semantic-query-text",
            "where the needle is kept",
            "--top-k",
            "10",
        ]);
        assert!(parsed.is_ok(), "{parsed:?}");
        let Ok(parsed) = parsed else {
            return;
        };
        let CliRequest::Explain {
            candidate,
            text_query,
            semantic_query_text,
            ..
        } = parsed.request
        else {
            panic!("expected explain payload");
        };
        assert_eq!(
            candidate,
            ExplainCandidateV1::Hybrid(sample_hybrid_candidate())
        );
        let Some(text_query) = text_query else {
            panic!("a hybrid explain carries its text query");
        };
        assert_eq!(text_query.query_text, "needle");
        assert_eq!(
            text_query.top_k, 10,
            "the fused top_k sizes the re-run lanes"
        );
        assert_eq!(
            semantic_query_text.as_deref(),
            Some("where the needle is kept")
        );

        // Each of the three hybrid requirements is refused by name.
        for (dropped, flag) in [
            (
                vec!["--semantic-query-text", "x", "--top-k", "10"],
                "--query-text",
            ),
            (
                vec![
                    "--syntax",
                    "native",
                    "--query-text",
                    "needle",
                    "--top-k",
                    "10",
                ],
                "--semantic-query-text",
            ),
            (
                vec![
                    "--syntax",
                    "native",
                    "--query-text",
                    "needle",
                    "--semantic-query-text",
                    "x",
                ],
                "--top-k",
            ),
        ] {
            let mut args = vec![
                "explain",
                "--repo-id",
                "repo",
                "--revision-id",
                "rev",
                "--manifest-generation",
                "7",
                "--hybrid-candidate-json",
                path.as_str(),
            ];
            args.extend(dropped);
            let refused = ParsedCommand::parse(args);
            let Err(error) = refused else {
                panic!("a hybrid explain without {flag} must be refused");
            };
            assert_eq!(error.exit_code, EXIT_USAGE);
            assert!(error.message.contains(flag), "{}", error.message);
        }

        let both = ParsedCommand::parse([
            "explain",
            "--repo-id",
            "repo",
            "--revision-id",
            "rev",
            "--manifest-generation",
            "7",
            "--candidate-json",
            path.as_str(),
            "--hybrid-candidate-json",
            path.as_str(),
        ]);
        assert!(both.is_err());
        if let Err(error) = both {
            assert_eq!(error.exit_code, EXIT_USAGE);
            assert!(
                error.message.contains("mutually exclusive"),
                "{}",
                error.message
            );
        }
    }
    /// A keyset route continues from the cursor a previous page printed
    /// as JSON (QI-BB-025 W4).
    ///
    /// `--cursor-json` carries the opaque token unchanged. Binding to a
    /// route is verified by the daemon, not by the CLI JSON parser.
    #[test]
    fn parses_keyset_routes_with_a_cursor_json_continuation() {
        let dir = tempfile::tempdir();
        assert!(dir.is_ok());
        let Ok(dir) = dir else {
            return;
        };
        let runtime_cursor = ContinuationTokenV2::new("runtime-token".to_string()).expect("token");
        let structural_cursor =
            ContinuationTokenV2::new("structural-token".to_string()).expect("token");
        let history_cursor = ContinuationTokenV2::new("history-token".to_string()).expect("token");
        let write = |name: &str, json: Result<Vec<u8>, serde_json::Error>| -> String {
            let path = dir.path().join(name);
            let written = json
                .map_err(|err| err.to_string())
                .and_then(|bytes| fs::write(&path, bytes).map_err(|err| err.to_string()));
            assert!(written.is_ok(), "{written:?}");
            path.to_string_lossy().into_owned()
        };
        let runtime_path = write(
            "runtime-cursor.json",
            serde_json::to_vec_pretty(&runtime_cursor),
        );
        let structural_path = write(
            "structural-cursor.json",
            serde_json::to_vec_pretty(&structural_cursor),
        );
        let history_path = write(
            "history-cursor.json",
            serde_json::to_vec_pretty(&history_cursor),
        );
        let malformed_path = dir.path().join("malformed.json");
        let written = fs::write(&malformed_path, b"{ not json");
        assert!(written.is_ok(), "{written:?}");
        let malformed_path = malformed_path.to_string_lossy().into_owned();

        let common = [
            "--repo-id",
            "repo",
            "--revision-id",
            "rev",
            "--manifest-generation",
            "9",
            "--syntax",
            "native",
            "--query-text",
            "dirty:yes needle",
            "--top-k",
            "10",
        ];
        let with_cursor = |command: &str, path: &str| {
            let mut args = vec![command.to_string()];
            args.extend(common.iter().map(ToString::to_string));
            if command == "history" {
                // The history route names its order; a recency cursor
                // continues a recency walk.
                args.push("--order".to_string());
                args.push("recency".to_string());
            }
            args.push("--cursor-json".to_string());
            args.push(path.to_string());
            ParsedCommand::parse(args)
        };

        let parsed = with_cursor("runtime-metadata", &runtime_path);
        assert!(parsed.is_ok(), "{parsed:?}");
        if let Ok(parsed) = parsed {
            let CliRequest::RuntimeMetadata(request) = parsed.request else {
                panic!("expected runtime-metadata payload");
            };
            assert_eq!(request.cursor, Some(runtime_cursor));
        }
        let parsed = with_cursor("structural", &structural_path);
        assert!(parsed.is_ok(), "{parsed:?}");
        if let Ok(parsed) = parsed {
            let CliRequest::Structural(request) = parsed.request else {
                panic!("expected structural payload");
            };
            assert_eq!(request.cursor, Some(structural_cursor));
        }
        let parsed = with_cursor("history", &history_path);
        assert!(parsed.is_ok(), "{parsed:?}");
        if let Ok(parsed) = parsed {
            let CliRequest::History(request) = parsed.request else {
                panic!("expected history payload");
            };
            assert_eq!(request.cursor, Some(history_cursor));
        }

        // A fresh walk carries no cursor.
        let mut fresh = vec!["structural".to_string()];
        fresh.extend(common.iter().map(ToString::to_string));
        let parsed = ParsedCommand::parse(fresh);
        assert!(parsed.is_ok(), "{parsed:?}");
        if let Ok(parsed) = parsed {
            let CliRequest::Structural(request) = parsed.request else {
                panic!("expected structural payload");
            };
            assert_eq!(request.cursor, None);
        }

        // Route binding is server-side; CLI must not inspect an opaque token.
        assert!(with_cursor("structural", &runtime_path).is_ok());
        assert!(with_cursor("runtime-metadata", &structural_path).is_ok());
        // A malformed JSON document is still a local usage error.
        let (command, path) = ("history", malformed_path.as_str());
        let parsed = with_cursor(command, path);
        assert!(parsed.is_err(), "{command}: {parsed:?}");
        if let Err(error) = parsed {
            assert_eq!(error.exit_code, EXIT_USAGE, "{command}: {error:?}");
            assert!(
                error
                    .message
                    .contains(&format!("failed to decode {command} cursor json")),
                "{command}: {}",
                error.message
            );
        }
    }
}
