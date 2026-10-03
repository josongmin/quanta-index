use crate::{CliError, CliResult, prometheus_is_metrics_only};
use quanta_index_contract::{
    ContinuationTokenV2, ExplainCandidateV1, GenerationPin, HistoryOrderV1, HistoryQueryRequest,
    HybridCandidateV1, HybridQueryRequest, HybridSeedQueryRequest, LexicalCandidate,
    ManifestGeneration, ProcessRequestEventPlaneV1, QueryConstraintSetV1, RepoId, RepoMapDocType,
    RepoMapFocusSubjectDto, RepoMapQueryRequest, RevisionId, RuntimeMetadataQueryRequest,
    SemanticQueryRequest, StructuralQueryRequest, SymbolQueryRequest, TextQueryRequest,
    TextQuerySyntax,
    ipc::{
        QuarantineTargetV1, QuarantinedGenerationEntryV1, QuarantinedRepoMapFileEntryV1,
        SearchPlaneTrackKind,
    },
};
use quanta_index_sdk::ConnectOptions;
use std::collections::VecDeque;
use std::fs;
use std::io::Read as _;
use std::path::PathBuf;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum OutputMode {
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

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum CommandKind {
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
    RequestEvents,
    GenerationStatus,
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
pub(super) enum CliRequest {
    Lexical(TextQueryRequest),
    Symbol(SymbolQueryRequest),
    Semantic(SemanticQueryRequest),
    /// QI-BB-018: two independent lanes fused by RRF.
    Hybrid(HybridQueryRequest),
    HybridSeed(HybridSeedQueryRequest),
    Explain {
        generation: GenerationPin,
        candidate: Box<ExplainCandidateV1>,
        text_query: Option<TextQueryRequest>,
        /// QI-BB-022: the dense query a hybrid row is re-derived under.
        semantic_query_text: Option<String>,
    },
    RepoMap(RepoMapQueryRequest),
    RuntimeMetadata(RuntimeMetadataQueryRequest),
    History(HistoryQueryRequest),
    Structural(StructuralQueryRequest),
    GenerationStatus {
        repo_id: RepoId,
        revision_id: RevisionId,
    },
    ProcessReadiness,
    RequestEvents {
        plane: ProcessRequestEventPlaneV1,
        limit: u16,
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
pub(super) struct ParsedCommand {
    pub(super) kind: CommandKind,
    pub(super) output: OutputMode,
    pub(super) connect_options: ConnectOptions,
    pub(super) request: CliRequest,
}

impl ParsedCommand {
    pub(super) fn parse<I, T>(args: I) -> CliResult<Self>
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
                parse_process_readiness(&mut common, &mut rest)?,
            ),
            "events" => (
                CommandKind::RequestEvents,
                parse_request_events(&mut common, &mut rest)?,
            ),
            "generation-status" => (
                CommandKind::GenerationStatus,
                parse_generation_status(&mut common, &mut rest)?,
            ),
            "doctor" => (CommandKind::Doctor, parse_doctor(&mut common, &mut rest)?),
            "metrics" => (CommandKind::Metrics, parse_metrics(&mut common, &mut rest)?),
            "quarantine" => (
                CommandKind::Quarantine,
                parse_quarantine(&mut common, &mut rest)?,
            ),
            other => {
                return Err(CliError::usage(format!(
                    "unknown subcommand `{other}`; expected lexical|symbol|semantic|hybrid|hybrid-seed|explain|repomap|runtime-metadata|history|structural|readiness|events|generation-status|doctor|metrics|quarantine"
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

/// Parse `<command> [--syntax] --query-text --top-k [--cursor-json PATH|-]`
/// with the pinned-generation flags.
///
/// Only `lexical` defaults to code search; other routes require a syntax.
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
    let syntax = match (command, syntax) {
        ("lexical", None) => TextQuerySyntax::CodeSearch,
        (_, Some(syntax)) => syntax,
        (_, None) => return Err(CliError::usage("missing --syntax".to_string())),
    };
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

/// Per-repository generation status, intentionally not process readiness.
fn parse_generation_status(
    common: &mut CommonOptions,
    rest: &mut VecDeque<String>,
) -> CliResult<CliRequest> {
    let (repo_id, revision_id) = parse_repo_revision(common, rest, "generation-status")?;
    Ok(CliRequest::GenerationStatus {
        repo_id,
        revision_id,
    })
}

fn parse_process_readiness(
    common: &mut CommonOptions,
    rest: &mut VecDeque<String>,
) -> CliResult<CliRequest> {
    while let Some(current) = rest.pop_front() {
        if common.parse_flag(&current, rest)? {
            continue;
        }
        return Err(CliError::usage(format!(
            "unknown readiness flag `{current}`"
        )));
    }
    Ok(CliRequest::ProcessReadiness)
}

fn parse_request_events(
    common: &mut CommonOptions,
    rest: &mut VecDeque<String>,
) -> CliResult<CliRequest> {
    let mut plane = None;
    let mut limit = None;
    while let Some(current) = rest.pop_front() {
        if common.parse_flag(&current, rest)? {
            continue;
        }
        match current.as_str() {
            "--plane" if plane.is_none() => {
                plane = Some(match take_value(rest, "--plane")?.as_str() {
                    "query" => ProcessRequestEventPlaneV1::Query,
                    "control" => ProcessRequestEventPlaneV1::Control,
                    "ingest" => ProcessRequestEventPlaneV1::Ingest,
                    other => return Err(CliError::usage(format!("invalid event plane `{other}`"))),
                });
            }
            "--limit" if limit.is_none() => {
                let raw = take_value(rest, "--limit")?;
                let parsed = raw.parse::<u16>().map_err(|error| {
                    CliError::usage(format!("invalid event limit `{raw}`: {error}"))
                })?;
                if parsed == 0 || parsed > quanta_index_contract::MAX_PROCESS_REQUEST_EVENTS_V1 {
                    return Err(CliError::usage("event limit must be 1..=1024".to_owned()));
                }
                limit = Some(parsed);
            }
            _ => {
                return Err(CliError::usage(format!(
                    "unknown or duplicate events flag `{current}`"
                )));
            }
        }
    }
    Ok(CliRequest::RequestEvents {
        plane: plane.ok_or_else(|| CliError::usage("events requires --plane".to_owned()))?,
        limit: limit.ok_or_else(|| CliError::usage("events requires --limit".to_owned()))?,
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
        candidate: Box::new(candidate),
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
            "unsupported syntax `{value}`; expected `code_search`, `native` or `sourcegraph`"
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

pub(super) fn parse_focus_subject(value: &str) -> CliResult<RepoMapFocusSubjectDto> {
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
