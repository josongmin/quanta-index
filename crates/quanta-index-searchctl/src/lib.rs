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
    EarlyStopReason, EngineTouched, GenerationPin, HistoryQueryRequest, HybridSeedQueryRequest,
    LexicalCandidate, ManifestGeneration, PlannerTraceEntry, QueryErrorRepair, RepoId,
    RepoMapDocType, RepoMapFocusSubjectDto, RepoMapQueryRequest, RevisionId,
    RuntimeMetadataQueryRequest, SearchExplanation, SearchPlaneHistoryQueryResponse,
    SearchPlaneQueryIpcResponse, SearchPlaneQueryIpcResponseEnvelope,
    SearchPlaneRuntimeMetadataQueryResponse, SearchPlaneStructuralQueryResponse,
    SemanticQueryRequest, StructuralQueryRequest, SymbolCandidate, SymbolQueryRequest,
    SymbolQueryResponse, TextQueryRequest, TextQueryResponse, TextQuerySyntax,
    ipc::GenerationStatusReport,
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
    // J7Q-05: readiness dispatches via the CONTROL plane, not the query plane.
    // Branch here so the query path (`dispatch_query_request` ->
    // `validate_response_kind` -> `render_response`) stays untouched.
    if let CliRequest::Readiness {
        repo_id,
        revision_id,
    } = request
    {
        let report = client
            .generations()
            .status(repo_id, revision_id)
            .map_err(map_sdk_error)?;
        let rendered = render_readiness(&report, output)?;
        stdout
            .write_all(rendered.as_bytes())
            .map_err(|err| CliError::transport(format!("failed writing stdout: {err}")))?;
        return Ok(ExitCode::SUCCESS);
    }
    let response = dispatch_query_request(&client, request)?;
    validate_response_kind(kind, &response)?;
    render_response(output, &response, stdout)?;
    Ok(ExitCode::SUCCESS)
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum OutputMode {
    Pretty,
    Json,
}

impl OutputMode {
    fn parse(value: &str) -> CliResult<Self> {
        match value {
            "pretty" => Ok(Self::Pretty),
            "json" => Ok(Self::Json),
            other => Err(CliError::usage(format!(
                "unsupported output mode `{other}`; expected `pretty` or `json`"
            ))),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum CommandKind {
    Lexical,
    Symbol,
    Semantic,
    HybridSeed,
    Explain,
    RepoMap,
    RuntimeMetadata,
    History,
    Structural,
    Readiness,
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
    HybridSeed(HybridSeedQueryRequest),
    Explain {
        generation: GenerationPin,
        candidate: LexicalCandidate,
    },
    RepoMap(RepoMapQueryRequest),
    RuntimeMetadata(RuntimeMetadataQueryRequest),
    History(HistoryQueryRequest),
    Structural(StructuralQueryRequest),
    Readiness {
        repo_id: RepoId,
        revision_id: RevisionId,
    },
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
            "hybrid" => {
                return Err(CliError::usage(
                    "subcommand `hybrid` was retired; use `hybrid-seed` or the Semantica hybrid rerank surface"
                        .to_string(),
                ));
            }
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
            other => {
                return Err(CliError::usage(format!(
                    "unknown subcommand `{other}`; expected lexical|symbol|semantic|hybrid-seed|explain|repomap|runtime-metadata|history|structural|readiness"
                )));
            }
        };
        if !rest.is_empty() {
            let extra = rest.pop_front().unwrap_or_default();
            return Err(CliError::usage(format!(
                "unexpected trailing argument `{extra}`"
            )));
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
    let mut generation_args = PinnedGenerationArgs::default();
    let mut syntax: Option<TextQuerySyntax> = None;
    let mut query_text: Option<String> = None;
    let mut top_k: Option<u32> = None;
    parse_query_command_flags(
        common,
        &mut generation_args,
        rest,
        "lexical",
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
            _ => Ok(false),
        },
    )?;
    let generation = generation_args.into_generation_pin()?;
    let syntax = syntax.ok_or_else(|| CliError::usage("missing --syntax".to_string()))?;
    let query_text =
        query_text.ok_or_else(|| CliError::usage("missing --query-text".to_string()))?;
    let top_k = top_k.ok_or_else(|| CliError::usage("missing --top-k".to_string()))?;
    Ok(CliRequest::Lexical(TextQueryRequest {
        syntax,
        query_text,
        generation: Some(generation),
        generation_selector: None,
        top_k,
    }))
}

fn parse_symbol(common: &mut CommonOptions, rest: &mut VecDeque<String>) -> CliResult<CliRequest> {
    let mut generation_args = PinnedGenerationArgs::default();
    let mut syntax: Option<TextQuerySyntax> = None;
    let mut query_text: Option<String> = None;
    let mut top_k: Option<u32> = None;
    parse_query_command_flags(
        common,
        &mut generation_args,
        rest,
        "symbol",
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
            _ => Ok(false),
        },
    )?;
    let generation = generation_args.into_generation_pin()?;
    let syntax = syntax.ok_or_else(|| CliError::usage("missing --syntax".to_string()))?;
    let query_text =
        query_text.ok_or_else(|| CliError::usage("missing --query-text".to_string()))?;
    let top_k = top_k.ok_or_else(|| CliError::usage("missing --top-k".to_string()))?;
    Ok(CliRequest::Symbol(SymbolQueryRequest {
        syntax,
        query_text,
        generation: Some(generation),
        generation_selector: None,
        top_k,
    }))
}

fn parse_runtime_metadata(
    common: &mut CommonOptions,
    rest: &mut VecDeque<String>,
) -> CliResult<CliRequest> {
    let mut generation_args = PinnedGenerationArgs::default();
    let mut syntax: Option<TextQuerySyntax> = None;
    let mut query_text: Option<String> = None;
    let mut top_k: Option<u32> = None;
    parse_query_command_flags(
        common,
        &mut generation_args,
        rest,
        "runtime-metadata",
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
            _ => Ok(false),
        },
    )?;
    let generation = generation_args.into_generation_pin()?;
    let syntax = syntax.ok_or_else(|| CliError::usage("missing --syntax".to_string()))?;
    let query_text =
        query_text.ok_or_else(|| CliError::usage("missing --query-text".to_string()))?;
    let top_k = top_k.ok_or_else(|| CliError::usage("missing --top-k".to_string()))?;
    let text_query = TextQueryRequest {
        syntax,
        query_text,
        generation: Some(generation),
        generation_selector: None,
        top_k,
    };
    Ok(CliRequest::RuntimeMetadata(RuntimeMetadataQueryRequest {
        text_query,
    }))
}

fn parse_text_query_wrapper(
    common: &mut CommonOptions,
    rest: &mut VecDeque<String>,
    command: &str,
) -> CliResult<TextQueryRequest> {
    let mut generation_args = PinnedGenerationArgs::default();
    let mut syntax: Option<TextQuerySyntax> = None;
    let mut query_text: Option<String> = None;
    let mut top_k: Option<u32> = None;
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
            _ => Ok(false),
        },
    )?;
    let generation = generation_args.into_generation_pin()?;
    let syntax = syntax.ok_or_else(|| CliError::usage("missing --syntax".to_string()))?;
    let query_text =
        query_text.ok_or_else(|| CliError::usage("missing --query-text".to_string()))?;
    let top_k = top_k.ok_or_else(|| CliError::usage("missing --top-k".to_string()))?;
    Ok(TextQueryRequest {
        syntax,
        query_text,
        generation: Some(generation),
        generation_selector: None,
        top_k,
    })
}

fn parse_history(common: &mut CommonOptions, rest: &mut VecDeque<String>) -> CliResult<CliRequest> {
    let text_query = parse_text_query_wrapper(common, rest, "history")?;
    Ok(CliRequest::History(HistoryQueryRequest { text_query }))
}

fn parse_structural(
    common: &mut CommonOptions,
    rest: &mut VecDeque<String>,
) -> CliResult<CliRequest> {
    let text_query = parse_text_query_wrapper(common, rest, "structural")?;
    Ok(CliRequest::Structural(StructuralQueryRequest {
        text_query,
    }))
}

/// J7Q-05: parse `readiness --repo-id <ID> --revision-id <REV>`.
///
/// Mirrors [`parse_history`]'s flag-loop shape but resolves to a
/// `(repo, revision)` pair only — readiness has no generation pin, syntax, or
/// `top-k` because it queries the activation catalog, not a sealed generation.
/// Missing `--repo-id` / `--revision-id` are rejected fail-closed (`EXIT_USAGE`),
/// never defaulted.
fn parse_readiness(
    common: &mut CommonOptions,
    rest: &mut VecDeque<String>,
) -> CliResult<CliRequest> {
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
                return Err(CliError::usage(format!("unknown readiness flag `{other}`")));
            }
        }
    }
    Ok(CliRequest::Readiness {
        repo_id: RepoId::new(
            repo_id.ok_or_else(|| CliError::usage("missing --repo-id".to_string()))?,
        ),
        revision_id: RevisionId::new(
            revision_id.ok_or_else(|| CliError::usage("missing --revision-id".to_string()))?,
        ),
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
                generation: Some(generation.clone()),
                generation_selector: None,
                top_k: scope_top_k,
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
        generation: Some(generation),
        generation_selector: None,
        lexical_scope,
        top_k: top_k.ok_or_else(|| CliError::usage("missing --top-k".to_string()))?,
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
        generation: Some(generation.clone()),
        generation_selector: None,
        top_k: text_query_top_k,
    };
    Ok(CliRequest::HybridSeed(HybridSeedQueryRequest {
        text_query,
        semantic_query_text: semantic_query_text
            .ok_or_else(|| CliError::usage("missing --semantic-query".to_string()))?,
        generation: Some(generation),
        generation_selector: None,
        top_k: top_k.ok_or_else(|| CliError::usage("missing --top-k".to_string()))?,
    }))
}

fn parse_explain(common: &mut CommonOptions, rest: &mut VecDeque<String>) -> CliResult<CliRequest> {
    let mut generation_args = PinnedGenerationArgs::default();
    let mut candidate_json: Option<String> = None;
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
            _ => Ok(false),
        },
    )?;
    let generation = generation_args.into_generation_pin()?;
    let candidate_path =
        candidate_json.ok_or_else(|| CliError::usage("missing --candidate-json".to_string()))?;
    Ok(CliRequest::Explain {
        generation,
        candidate: read_candidate_json(&candidate_path)?,
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
        RepoId::new(repo_id.ok_or_else(|| CliError::usage("missing --repo-id".to_string()))?),
        RevisionId::new(
            revision_id.ok_or_else(|| CliError::usage("missing --revision-id".to_string()))?,
        ),
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
        } => SearchPlaneQueryIpcResponse::Explain(
            client
                .search()
                .explain(generation, candidate)
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
        // Readiness is a control-plane command; `run_inner` branches it before
        // reaching the query dispatcher. Reaching here is a routing bug, so
        // fail-closed with a typed protocol error rather than fabricate a query.
        CliRequest::Readiness { .. } => {
            return Err(CliError::protocol(
                "readiness is a control-plane command and must not reach the query dispatcher"
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
    let raw = if candidate_path == "-" {
        let mut input = String::new();
        let _bytes_read = std::io::stdin().read_to_string(&mut input).map_err(|err| {
            CliError::transport(format!("failed reading candidate json from stdin: {err}"))
        })?;
        input
    } else {
        fs::read_to_string(candidate_path).map_err(|err| {
            CliError::transport(format!(
                "failed reading candidate json from {candidate_path}: {err}"
            ))
        })?
    };
    serde_json::from_str::<LexicalCandidate>(&raw).map_err(|err| {
        CliError::usage(format!(
            "failed to decode candidate json from {candidate_path}: {err}"
        ))
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
            Ok(rendered)
        }
    }
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
                file_owner_rows: None,
            },
            Some(&payload.explanation),
            rendered,
        ),
        SearchPlaneQueryIpcResponse::Hybrid(payload) => render_lexical_payload(
            "hybrid-internal",
            &TextQueryResponse {
                generation: payload.generation.clone(),
                results: payload.results.clone(),
                file_owner_rows: None,
            },
            Some(&payload.explanation),
            rendered,
        ),
        SearchPlaneQueryIpcResponse::HybridSeed(payload) => render_lexical_payload(
            "hybrid-seed",
            &TextQueryResponse {
                generation: payload.generation.clone(),
                results: payload
                    .seed_candidates
                    .iter()
                    .map(|candidate| candidate.candidate.clone())
                    .collect(),
                file_owner_rows: None,
            },
            Some(&payload.explanation),
            rendered,
        ),
        SearchPlaneQueryIpcResponse::Explain(payload) => {
            fmt_ok(writeln!(rendered, "kind: explain"))?;
            render_generation(&payload.generation, rendered)?;
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
                    "{}. subject_identity={} doc_type={} kind={} owner_path={} rank={} included={} score={} final_score_millis={}",
                    display_index,
                    entry.subject_identity,
                    entry.subject_doc_type.as_code_str(),
                    entry.subject_kind,
                    entry.owner_path,
                    entry.rank,
                    entry.included,
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
    }
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
    for (index, commit) in payload.commits.iter().enumerate() {
        let display_index = index
            .checked_add(1)
            .ok_or_else(|| CliError::protocol("commit index overflow".to_string()))?;
        fmt_ok(writeln!(
            rendered,
            "{}. sha={} author={} committer={} committed_at_unix_s={} is_merge={} tags={}",
            display_index,
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
            "{}. path={} hunk_header={} side={} lines={}-{}",
            display_index,
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

fn render_structural_payload(
    payload: &SearchPlaneStructuralQueryResponse,
    rendered: &mut String,
) -> CliResult<()> {
    fmt_ok(writeln!(rendered, "kind: structural"))?;
    render_generation(&payload.generation, rendered)?;
    fmt_ok(writeln!(rendered, "results: {}", payload.results.len()))?;
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
    }
}

fn command_kind_name(kind: CommandKind) -> &'static str {
    match kind {
        CommandKind::Lexical => "lexical",
        CommandKind::Symbol => "symbol",
        CommandKind::Semantic => "semantic",
        CommandKind::HybridSeed => "hybrid-seed",
        CommandKind::Explain => "explain",
        CommandKind::RepoMap => "repomap",
        CommandKind::RuntimeMetadata => "runtime-metadata",
        CommandKind::History => "history",
        CommandKind::Structural => "structural",
        CommandKind::Readiness => "readiness",
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
    code: &str,
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
  --output pretty|json

Read-only subcommands:
  lexical          --repo-id ID --revision-id REV --manifest-generation N --syntax native|sourcegraph --query-text TEXT --top-k N
  symbol           --repo-id ID --revision-id REV --manifest-generation N --syntax native|sourcegraph --query-text TEXT --top-k N
  semantic         --repo-id ID --revision-id REV --manifest-generation N --query-text TEXT --top-k N [--scope-query TEXT --scope-syntax native|sourcegraph --scope-top-k N]
  hybrid-seed      --repo-id ID --revision-id REV --manifest-generation N --lexical-query TEXT --lexical-syntax native|sourcegraph --semantic-query TEXT --top-k N
  explain          --repo-id ID --revision-id REV --manifest-generation N --candidate-json PATH|-
  repomap          --repo-id ID --revision-id REV --manifest-generation N --query-text TEXT --top-k N --token-budget N [--focus-subject subject_identity:subject_doc_type]
  runtime-metadata --repo-id ID --revision-id REV --manifest-generation N --syntax native|sourcegraph --query-text TEXT --top-k N
  history          --repo-id ID --revision-id REV --manifest-generation N --syntax native|sourcegraph --query-text TEXT --top-k N
  structural       --repo-id ID --revision-id REV --manifest-generation N --syntax native|sourcegraph --query-text TEXT --top-k N
  readiness        --repo-id ID --revision-id REV
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

    #[test]
    fn rejects_retired_hybrid_subcommand() {
        let parsed = ParsedCommand::parse([
            "hybrid",
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
        assert!(parsed.is_err());
        let Err(error) = parsed else {
            return;
        };
        assert_eq!(error.exit_code, EXIT_USAGE);
        assert!(error.message.contains("retired"));
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
                    RepoId::new("repo"),
                    RevisionId::new("rev"),
                    ManifestGeneration::new(7),
                ),
                results: vec![SymbolCandidate {
                    candidate_id: "sym-1".to_string(),
                    repo_id: RepoId::new("repo"),
                    revision_id: RevisionId::new("rev"),
                    manifest_generation: ManifestGeneration::new(7),
                    repo_relative_path: RepoRelativePath::new("src/lib.rs"),
                    start_line: 10,
                    end_line: 12,
                    score: 0.8,
                    snippet: "fn my_symbol() {}".to_string(),
                    symbol_kind,
                    symbol_kind_family: Some(SymbolKindFamily::Callable),
                }],
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
        let response = SearchPlaneQueryIpcResponseEnvelope {
            request_id: 1,
            payload: SearchPlaneQueryIpcResponse::RuntimeMetadata(
                SearchPlaneRuntimeMetadataQueryResponse {
                    generation: GenerationPin::new(
                        RepoId::new("repo"),
                        RevisionId::new("rev"),
                        ManifestGeneration::new(7),
                    ),
                    results: vec![LexicalCandidate {
                        candidate_id: "rt-1".to_string(),
                        repo_id: RepoId::new("repo"),
                        revision_id: RevisionId::new("rev"),
                        manifest_generation: ManifestGeneration::new(7),
                        repo_relative_path: RepoRelativePath::new("src/runtime.rs"),
                        start_line: 1,
                        end_line: 4,
                        score: 0.3,
                        snippet: "runtime body".to_string(),
                        snippet_hit_offset: None,
                    }],
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
        }
    }

    #[test]
    fn pretty_renderer_supports_sourcegraph_text_response() {
        let response = SearchPlaneQueryIpcResponseEnvelope {
            request_id: 1,
            payload: SearchPlaneQueryIpcResponse::Text(TextQueryResponse {
                generation: GenerationPin::new(
                    RepoId::new("repo"),
                    RevisionId::new("rev"),
                    ManifestGeneration::new(7),
                ),
                results: vec![LexicalCandidate {
                    candidate_id: "cand-1".to_string(),
                    repo_id: RepoId::new("repo"),
                    revision_id: RevisionId::new("rev"),
                    manifest_generation: ManifestGeneration::new(7),
                    repo_relative_path: quanta_index_contract::RepoRelativePath::new("src/lib.rs"),
                    start_line: 1,
                    end_line: 3,
                    score: 0.5,
                    snippet: "fn sample() {}".to_string(),
                    snippet_hit_offset: None,
                }],
                file_owner_rows: Some(vec![quanta_index_contract::FileOwnerProjectionRow {
                    candidate_id: "cand-1".to_string(),
                    repo_id: RepoId::new("repo"),
                    revision_id: RevisionId::new("rev"),
                    manifest_generation: ManifestGeneration::new(7),
                    repo_relative_path: quanta_index_contract::RepoRelativePath::new("src/lib.rs"),
                    owners: vec!["@alice".to_string(), "@acme/platform".to_string()],
                }]),
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
        assert_eq!(
            request
                .text_query
                .generation
                .map(|pin| pin.manifest_generation.get()),
            Some(7)
        );
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
                    RepoId::new("repo"),
                    RevisionId::new("rev"),
                    ManifestGeneration::new(7),
                ),
                commits: vec![quanta_index_contract::CommitCandidate {
                    sha: quanta_index_contract::lex::CommitSha::ZERO,
                    parent_ids: Vec::new(),
                    committed_at_unix_s: 1_700_000_000,
                    author: "alice".to_string(),
                    committer: "alice".to_string(),
                    message: "fix: thing\nbody line".to_string(),
                    is_merge: false,
                    tags: vec!["v1.0".to_string()],
                }],
                diffs: vec![quanta_index_contract::DiffCandidate {
                    repo_relative_path: "src/lib.rs".to_string(),
                    hunk_header: "@@ -1,3 +1,4 @@".to_string(),
                    side: quanta_index_contract::DiffHunkSide::After,
                    line_start: 1,
                    line_end: 4,
                    snippet: "fn sample() {}".to_string(),
                }],
            }),
        };
        let mut stdout = Vec::new();
        let rendered = render_response(OutputMode::Pretty, &response, &mut stdout);
        assert!(rendered.is_ok());
        let text = String::from_utf8(stdout);
        assert!(text.is_ok());
        if let Ok(text) = text {
            assert!(text.contains("kind: history"));
            assert!(text.contains("commits: 1 diffs: 1"));
            assert!(text.contains("author=alice"));
            assert!(text.contains("path=src/lib.rs"));
            assert!(text.contains("side=after"));
        }
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

    #[test]
    fn render_readiness_json_emits_report_shape() {
        use quanta_index_contract::ipc::{
            GenerationStatusReport, SearchPlaneTrackKind, TrackReadinessRecord,
        };
        let report = GenerationStatusReport {
            repo_id: RepoId::new("repo-1"),
            revision_id: RevisionId::new("rev-1"),
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
            repo_id: RepoId::new("repo-1"),
            revision_id: RevisionId::new("rev-1"),
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
            repo_id: RepoId::new("repo-1"),
            revision_id: RevisionId::new("rev-1"),
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
                    RepoId::new("repo"),
                    RevisionId::new("rev"),
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
            }),
        };
        let mut stdout = Vec::new();
        let rendered = render_response(OutputMode::Pretty, &response, &mut stdout);
        assert!(rendered.is_ok());
        let text = String::from_utf8(stdout);
        assert!(text.is_ok());
        if let Ok(text) = text {
            assert!(text.contains("kind: structural"));
            assert!(text.contains("results: 1"));
            assert!(text.contains("candidate_id=struct-1"));
            assert!(text.contains("$NAME: bytes=10-14"));
        }
    }
}
