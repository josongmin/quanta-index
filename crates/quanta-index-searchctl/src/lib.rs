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
    EarlyStopReason, EngineTouched, GenerationPin, HybridQueryRequest, LexicalCandidate,
    ManifestGeneration, PlannerTraceEntry, RepoId, RepoMapDocType, RepoMapFocusSubjectDto,
    RepoMapQueryRequest, RevisionId, SearchExplanation, SearchPlaneExplainQueryRequest,
    SearchPlaneQueryIpcRequest, SearchPlaneQueryIpcRequestEnvelope, SearchPlaneQueryIpcResponse,
    SearchPlaneQueryIpcResponseEnvelope, SearchPlaneSourcegraphQueryRequest, SemanticQueryRequest,
    SemanticVectorRef, TextQueryRequest, TextQueryResponse, TextQuerySyntax,
};
use quanta_index_ipc::send_request;

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
    let parsed = ParsedCommand::parse(args)?;
    let response: SearchPlaneQueryIpcResponseEnvelope =
        send_request(&parsed.socket_path, &parsed.request)
            .map_err(|error| CliError::transport(format!("ipc request failed: {error}")))?;
    if response.request_id != parsed.request.request_id {
        return Err(CliError::protocol(format!(
            "response request_id {} != request {}",
            response.request_id, parsed.request.request_id
        )));
    }
    validate_response_kind(parsed.kind, &response)?;
    render_response(parsed.output, &response, stdout)?;
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
    Sourcegraph,
    Semantic,
    Hybrid,
    Explain,
    RepoMap,
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

    fn resolve_socket_path(&self) -> CliResult<PathBuf> {
        if let Some(socket) = &self.socket_override {
            return Ok(socket.clone());
        }
        let state_root = if let Some(root) = &self.state_root_override {
            root.clone()
        } else if let Ok(explicit) = std::env::var("QUANTA_INDEX_STATE_ROOT") {
            PathBuf::from(explicit)
        } else if let Ok(cache_root) = std::env::var("QUANTA_INDEX_CACHE_ROOT") {
            PathBuf::from(cache_root).join("state")
        } else {
            let home = std::env::var("HOME").map_err(|_err| {
                CliError::usage(
                    "cannot resolve socket path: set --socket, --state-root, HOME, or QUANTA_INDEX_*"
                        .to_string(),
                )
            })?;
            #[cfg(target_os = "macos")]
            let default_root = PathBuf::from(home).join("Library/Caches/quanta-index/state");
            #[cfg(not(target_os = "macos"))]
            let default_root = PathBuf::from(home).join(".cache/quanta-index/state");
            default_root
        };
        Ok(state_root.join("search-plane").join("query.sock"))
    }
}

#[derive(Clone, Debug, PartialEq)]
struct ParsedCommand {
    kind: CommandKind,
    output: OutputMode,
    socket_path: PathBuf,
    request: SearchPlaneQueryIpcRequestEnvelope,
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
            "sourcegraph" => (
                CommandKind::Sourcegraph,
                parse_sourcegraph(&mut common, &mut rest)?,
            ),
            "semantic" => (
                CommandKind::Semantic,
                parse_semantic(&mut common, &mut rest)?,
            ),
            "hybrid" => (CommandKind::Hybrid, parse_hybrid(&mut common, &mut rest)?),
            "explain" => (CommandKind::Explain, parse_explain(&mut common, &mut rest)?),
            "repomap" | "repomap-query" => {
                (CommandKind::RepoMap, parse_repomap(&mut common, &mut rest)?)
            }
            other => {
                return Err(CliError::usage(format!(
                    "unknown subcommand `{other}`; expected lexical|sourcegraph|semantic|hybrid|explain|repomap"
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
            socket_path: common.resolve_socket_path()?,
            request: SearchPlaneQueryIpcRequestEnvelope {
                request_id: REQUEST_ID,
                payload,
            },
        })
    }
}

fn parse_lexical(
    common: &mut CommonOptions,
    rest: &mut VecDeque<String>,
) -> CliResult<SearchPlaneQueryIpcRequest> {
    let mut repo_id: Option<String> = None;
    let mut revision_id: Option<String> = None;
    let mut manifest_generation: Option<u64> = None;
    let mut syntax: Option<TextQuerySyntax> = None;
    let mut query_text: Option<String> = None;
    let mut top_k: Option<u32> = None;
    while let Some(current) = rest.pop_front() {
        if common.parse_flag(&current, rest)? {
            continue;
        }
        match current.as_str() {
            "--repo-id" => repo_id = Some(take_value(rest, "--repo-id")?),
            "--revision-id" => revision_id = Some(take_value(rest, "--revision-id")?),
            "--manifest-generation" => {
                manifest_generation = Some(parse_u64_flag(rest, "--manifest-generation")?);
            }
            "--syntax" => syntax = Some(parse_syntax(&take_value(rest, "--syntax")?)?),
            "--query-text" => query_text = Some(take_value(rest, "--query-text")?),
            "--top-k" => top_k = Some(parse_u32_flag(rest, "--top-k")?),
            other => return Err(CliError::usage(format!("unknown lexical flag `{other}`"))),
        }
    }
    let generation = parse_generation_pin(repo_id, revision_id, manifest_generation)?;
    let syntax = syntax.ok_or_else(|| CliError::usage("missing --syntax".to_string()))?;
    let query_text =
        query_text.ok_or_else(|| CliError::usage("missing --query-text".to_string()))?;
    let top_k = top_k.ok_or_else(|| CliError::usage("missing --top-k".to_string()))?;
    Ok(SearchPlaneQueryIpcRequest::Text(TextQueryRequest {
        syntax,
        query_text,
        generation: Some(generation),
        generation_selector: None,
        top_k,
    }))
}

fn parse_semantic(
    common: &mut CommonOptions,
    rest: &mut VecDeque<String>,
) -> CliResult<SearchPlaneQueryIpcRequest> {
    let mut repo_id: Option<String> = None;
    let mut revision_id: Option<String> = None;
    let mut manifest_generation: Option<u64> = None;
    let mut query_text: Option<String> = None;
    let mut query_vector: Option<Vec<f32>> = None;
    let mut query_vector_handle: Option<String> = None;
    let mut top_k: Option<u32> = None;
    let mut scope_query_text: Option<String> = None;
    let mut scope_syntax: Option<TextQuerySyntax> = None;
    let mut scope_top_k: Option<u32> = None;
    while let Some(current) = rest.pop_front() {
        if common.parse_flag(&current, rest)? {
            continue;
        }
        match current.as_str() {
            "--repo-id" => repo_id = Some(take_value(rest, "--repo-id")?),
            "--revision-id" => revision_id = Some(take_value(rest, "--revision-id")?),
            "--manifest-generation" => {
                manifest_generation = Some(parse_u64_flag(rest, "--manifest-generation")?);
            }
            "--query-text" => query_text = Some(take_value(rest, "--query-text")?),
            "--query-vector" => query_vector = Some(parse_f32_vector_flag(rest, "--query-vector")?),
            "--query-vector-handle" => {
                query_vector_handle = Some(take_value(rest, "--query-vector-handle")?);
            }
            "--top-k" => top_k = Some(parse_u32_flag(rest, "--top-k")?),
            "--scope-query" => scope_query_text = Some(take_value(rest, "--scope-query")?),
            "--scope-syntax" => {
                scope_syntax = Some(parse_syntax(&take_value(rest, "--scope-syntax")?)?);
            }
            "--scope-top-k" => scope_top_k = Some(parse_u32_flag(rest, "--scope-top-k")?),
            other => return Err(CliError::usage(format!("unknown semantic flag `{other}`"))),
        }
    }
    let generation = parse_generation_pin(repo_id, revision_id, manifest_generation)?;
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
    let (query_text, query_vector_ref) = resolve_semantic_input(
        query_text,
        query_vector,
        query_vector_handle,
        "--query-text",
        "--query-vector",
        "--query-vector-handle",
    )?;
    Ok(SearchPlaneQueryIpcRequest::Semantic(SemanticQueryRequest {
        query_text: (!query_text.is_empty()).then_some(query_text),
        query_vector: None,
        query_vector_ref,
        generation: Some(generation),
        generation_selector: None,
        lexical_scope,
        top_k: top_k.ok_or_else(|| CliError::usage("missing --top-k".to_string()))?,
    }))
}

fn parse_sourcegraph(
    common: &mut CommonOptions,
    rest: &mut VecDeque<String>,
) -> CliResult<SearchPlaneQueryIpcRequest> {
    let mut repo_id: Option<String> = None;
    let mut revision_id: Option<String> = None;
    let mut manifest_generation: Option<u64> = None;
    let mut source_syntax: Option<String> = None;
    let mut sg_version: Option<String> = None;
    let mut top_k: Option<u32> = None;
    while let Some(current) = rest.pop_front() {
        if common.parse_flag(&current, rest)? {
            continue;
        }
        match current.as_str() {
            "--repo-id" => repo_id = Some(take_value(rest, "--repo-id")?),
            "--revision-id" => revision_id = Some(take_value(rest, "--revision-id")?),
            "--manifest-generation" => {
                manifest_generation = Some(parse_u64_flag(rest, "--manifest-generation")?);
            }
            "--query-text" => source_syntax = Some(take_value(rest, "--query-text")?),
            "--sg-version" => sg_version = Some(take_value(rest, "--sg-version")?),
            "--top-k" => top_k = Some(parse_u32_flag(rest, "--top-k")?),
            other => {
                return Err(CliError::usage(format!(
                    "unknown sourcegraph flag `{other}`"
                )));
            }
        }
    }
    let generation = parse_generation_pin(repo_id, revision_id, manifest_generation)?;
    Ok(SearchPlaneQueryIpcRequest::Sourcegraph(
        SearchPlaneSourcegraphQueryRequest {
            source_syntax: source_syntax
                .ok_or_else(|| CliError::usage("missing --query-text".to_string()))?
                .into_boxed_str(),
            sg_version: sg_version
                .ok_or_else(|| CliError::usage("missing --sg-version".to_string()))?
                .into_boxed_str(),
            generation: Some(generation),
            top_k: top_k.ok_or_else(|| CliError::usage("missing --top-k".to_string()))?,
        },
    ))
}

fn parse_hybrid(
    common: &mut CommonOptions,
    rest: &mut VecDeque<String>,
) -> CliResult<SearchPlaneQueryIpcRequest> {
    let mut repo_id: Option<String> = None;
    let mut revision_id: Option<String> = None;
    let mut manifest_generation: Option<u64> = None;
    let mut lexical_query_text: Option<String> = None;
    let mut lexical_syntax: Option<TextQuerySyntax> = None;
    let mut semantic_query_text: Option<String> = None;
    let mut semantic_vector: Option<Vec<f32>> = None;
    let mut semantic_vector_handle: Option<String> = None;
    let mut top_k: Option<u32> = None;
    while let Some(current) = rest.pop_front() {
        if common.parse_flag(&current, rest)? {
            continue;
        }
        match current.as_str() {
            "--repo-id" => repo_id = Some(take_value(rest, "--repo-id")?),
            "--revision-id" => revision_id = Some(take_value(rest, "--revision-id")?),
            "--manifest-generation" => {
                manifest_generation = Some(parse_u64_flag(rest, "--manifest-generation")?);
            }
            "--lexical-query" => lexical_query_text = Some(take_value(rest, "--lexical-query")?),
            "--lexical-syntax" => {
                lexical_syntax = Some(parse_syntax(&take_value(rest, "--lexical-syntax")?)?);
            }
            "--semantic-query" => semantic_query_text = Some(take_value(rest, "--semantic-query")?),
            "--semantic-vector" => {
                semantic_vector = Some(parse_f32_vector_flag(rest, "--semantic-vector")?);
            }
            "--semantic-vector-handle" => {
                semantic_vector_handle = Some(take_value(rest, "--semantic-vector-handle")?);
            }
            "--top-k" => top_k = Some(parse_u32_flag(rest, "--top-k")?),
            other => return Err(CliError::usage(format!("unknown hybrid flag `{other}`"))),
        }
    }
    let generation = parse_generation_pin(repo_id, revision_id, manifest_generation)?;
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
    let (semantic_query_text, semantic_vector_ref) = resolve_semantic_input(
        semantic_query_text,
        semantic_vector,
        semantic_vector_handle,
        "--semantic-query",
        "--semantic-vector",
        "--semantic-vector-handle",
    )?;
    Ok(SearchPlaneQueryIpcRequest::Hybrid(HybridQueryRequest {
        text_query,
        semantic_query_text: (!semantic_query_text.is_empty()).then_some(semantic_query_text),
        semantic_vector: None,
        semantic_vector_ref,
        generation: Some(generation),
        generation_selector: None,
        top_k: top_k.ok_or_else(|| CliError::usage("missing --top-k".to_string()))?,
    }))
}

fn parse_explain(
    common: &mut CommonOptions,
    rest: &mut VecDeque<String>,
) -> CliResult<SearchPlaneQueryIpcRequest> {
    let mut repo_id: Option<String> = None;
    let mut revision_id: Option<String> = None;
    let mut manifest_generation: Option<u64> = None;
    let mut candidate_json: Option<String> = None;
    while let Some(current) = rest.pop_front() {
        if common.parse_flag(&current, rest)? {
            continue;
        }
        match current.as_str() {
            "--repo-id" => repo_id = Some(take_value(rest, "--repo-id")?),
            "--revision-id" => revision_id = Some(take_value(rest, "--revision-id")?),
            "--manifest-generation" => {
                manifest_generation = Some(parse_u64_flag(rest, "--manifest-generation")?);
            }
            "--candidate-json" => candidate_json = Some(take_value(rest, "--candidate-json")?),
            other => return Err(CliError::usage(format!("unknown explain flag `{other}`"))),
        }
    }
    let generation = parse_generation_pin(repo_id, revision_id, manifest_generation)?;
    let candidate_path =
        candidate_json.ok_or_else(|| CliError::usage("missing --candidate-json".to_string()))?;
    Ok(SearchPlaneQueryIpcRequest::Explain(
        SearchPlaneExplainQueryRequest {
            generation,
            candidate: read_candidate_json(&candidate_path)?,
        },
    ))
}

fn parse_repomap(
    common: &mut CommonOptions,
    rest: &mut VecDeque<String>,
) -> CliResult<SearchPlaneQueryIpcRequest> {
    let mut repo_id: Option<String> = None;
    let mut revision_id: Option<String> = None;
    let mut manifest_generation: Option<u64> = None;
    let mut query_text: Option<String> = None;
    let mut top_k: Option<u32> = None;
    let mut token_budget: Option<u32> = None;
    let mut focus_subjects: Vec<RepoMapFocusSubjectDto> = Vec::new();
    while let Some(current) = rest.pop_front() {
        if common.parse_flag(&current, rest)? {
            continue;
        }
        match current.as_str() {
            "--repo-id" => repo_id = Some(take_value(rest, "--repo-id")?),
            "--revision-id" => revision_id = Some(take_value(rest, "--revision-id")?),
            "--manifest-generation" => {
                manifest_generation = Some(parse_u64_flag(rest, "--manifest-generation")?);
            }
            "--query-text" => query_text = Some(take_value(rest, "--query-text")?),
            "--top-k" => top_k = Some(parse_u32_flag(rest, "--top-k")?),
            "--token-budget" => token_budget = Some(parse_u32_flag(rest, "--token-budget")?),
            "--focus-subject" => {
                focus_subjects.push(parse_focus_subject(&take_value(rest, "--focus-subject")?)?);
            }
            other => return Err(CliError::usage(format!("unknown repomap flag `{other}`"))),
        }
    }
    let generation = parse_generation_pin(repo_id, revision_id, manifest_generation)?;
    Ok(SearchPlaneQueryIpcRequest::RepoMapQuery(
        RepoMapQueryRequest {
            repo_id: generation.repo_id,
            revision_id: generation.revision_id,
            manifest_generation: generation.manifest_generation,
            query_text: query_text
                .ok_or_else(|| CliError::usage("missing --query-text".to_string()))?,
            top_k: top_k.ok_or_else(|| CliError::usage("missing --top-k".to_string()))?,
            token_budget: token_budget
                .ok_or_else(|| CliError::usage("missing --token-budget".to_string()))?,
            focus_subjects,
        },
    ))
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

fn parse_u64_flag(rest: &mut VecDeque<String>, flag: &str) -> CliResult<u64> {
    let value = take_value(rest, flag)?;
    value.parse::<u64>().map_err(|err| {
        CliError::usage(format!(
            "{flag} requires an unsigned integer, got `{value}`: {err}"
        ))
    })
}

fn parse_f32_vector_flag(rest: &mut VecDeque<String>, flag: &str) -> CliResult<Vec<f32>> {
    let value = take_value(rest, flag)?;
    parse_f32_vector(&value, flag)
}

fn parse_f32_vector(value: &str, flag: &str) -> CliResult<Vec<f32>> {
    let trimmed = value.trim();
    let vector = if trimmed.starts_with('[') {
        serde_json::from_str::<Vec<f32>>(trimmed).map_err(|err| {
            CliError::usage(format!(
                "{flag} requires a JSON array or comma-separated f32 list, got `{value}`: {err}"
            ))
        })?
    } else if trimmed.is_empty() {
        Vec::new()
    } else {
        trimmed
            .split(',')
            .map(|token| {
                let token = token.trim();
                if token.is_empty() {
                    return Err(CliError::usage(format!(
                        "{flag} contains an empty vector component in `{value}`"
                    )));
                }
                token.parse::<f32>().map_err(|err| {
                    CliError::usage(format!(
                        "{flag} requires finite f32 values, got component `{token}`: {err}"
                    ))
                })
            })
            .collect::<CliResult<Vec<f32>>>()?
    };
    validate_query_vector(&vector, flag)?;
    Ok(vector)
}

fn validate_query_vector(vector: &[f32], flag: &str) -> CliResult<()> {
    if vector.is_empty() {
        return Err(CliError::usage(format!(
            "{flag} requires at least one vector component"
        )));
    }
    let mut non_zero = false;
    for component in vector {
        if !component.is_finite() {
            return Err(CliError::usage(format!(
                "{flag} requires finite f32 values, got `{component}`"
            )));
        }
        if *component != 0.0 {
            non_zero = true;
        }
    }
    if !non_zero {
        return Err(CliError::usage(format!(
            "{flag} requires a non-zero vector"
        )));
    }
    Ok(())
}

fn resolve_semantic_input(
    text: Option<String>,
    vector: Option<Vec<f32>>,
    handle: Option<String>,
    text_flag: &str,
    vector_flag: &str,
    handle_flag: &str,
) -> CliResult<(String, Option<SemanticVectorRef>)> {
    match (text, vector, handle) {
        (Some(_), Some(_), _) | (Some(_), _, Some(_)) | (None, Some(_), Some(_)) => {
            Err(CliError::usage(format!(
                "provide exactly one of {text_flag}, {vector_flag}, or {handle_flag}"
            )))
        }
        (Some(text), None, None) => Ok((text, None)),
        (None, Some(vector), None) => Ok((
            encode_query_vector_text(&vector),
            Some(SemanticVectorRef::Inline(vector)),
        )),
        (None, None, Some(handle)) => {
            if handle.is_empty() {
                return Err(CliError::usage(format!(
                    "{handle_flag} requires a non-empty handle"
                )));
            }
            Ok((
                String::new(),
                Some(SemanticVectorRef::Handle(handle.into_boxed_str())),
            ))
        }
        (None, None, None) => Err(CliError::usage(format!(
            "missing {text_flag}, {vector_flag}, or {handle_flag}"
        ))),
    }
}

fn encode_query_vector_text(vector: &[f32]) -> String {
    vector
        .iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>()
        .join(" ")
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
        | (CommandKind::Sourcegraph, SearchPlaneQueryIpcResponse::Sourcegraph(_))
        | (CommandKind::Semantic, SearchPlaneQueryIpcResponse::Semantic(_))
        | (CommandKind::Hybrid, SearchPlaneQueryIpcResponse::Hybrid(_))
        | (CommandKind::Explain, SearchPlaneQueryIpcResponse::Explain(_))
        | (CommandKind::RepoMap, SearchPlaneQueryIpcResponse::RepoMapQuery(_)) => Ok(()),
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

fn render_pretty(
    response: &SearchPlaneQueryIpcResponseEnvelope,
    rendered: &mut String,
) -> CliResult<()> {
    fmt_ok(writeln!(rendered, "request_id: {}", response.request_id))?;
    match &response.payload {
        SearchPlaneQueryIpcResponse::Text(payload) => {
            render_lexical_payload("lexical", payload, None, rendered)
        }
        SearchPlaneQueryIpcResponse::Sourcegraph(payload) => render_lexical_payload(
            "sourcegraph",
            &TextQueryResponse {
                generation: payload.generation.clone(),
                results: payload.results.clone(),
            },
            None,
            rendered,
        ),
        SearchPlaneQueryIpcResponse::Symbol(_payload) => Err(CliError::protocol(
            "unsupported pretty renderer for response kind `Symbol`".to_string(),
        )),
        SearchPlaneQueryIpcResponse::Semantic(payload) => render_lexical_payload(
            "semantic",
            &TextQueryResponse {
                generation: payload.generation.clone(),
                results: payload.results.clone(),
            },
            Some(&payload.explanation),
            rendered,
        ),
        SearchPlaneQueryIpcResponse::Hybrid(payload) => render_lexical_payload(
            "hybrid",
            &TextQueryResponse {
                generation: payload.generation.clone(),
                results: payload.results.clone(),
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
        SearchPlaneQueryIpcResponse::Error(error) => Err(CliError::remote(format!(
            "{}: {}",
            error.code, error.message
        ))),
        SearchPlaneQueryIpcResponse::History(_)
        | SearchPlaneQueryIpcResponse::Structural(_)
        | SearchPlaneQueryIpcResponse::Bridge(_)
        | SearchPlaneQueryIpcResponse::RuntimeMetadata(_) => Err(CliError::protocol(format!(
            "unsupported pretty renderer for response kind `{}`",
            response_kind_name(&response.payload)
        ))),
    }
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
    if let Some(explanation) = explanation {
        render_explanation(explanation, rendered)?;
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
        SearchPlaneQueryIpcResponse::History(_) => "History",
        SearchPlaneQueryIpcResponse::Structural(_) => "Structural",
        SearchPlaneQueryIpcResponse::Bridge(_) => "Bridge",
        SearchPlaneQueryIpcResponse::RepoMapQuery(_) => "RepoMapQuery",
        SearchPlaneQueryIpcResponse::Explain(_) => "Explain",
        SearchPlaneQueryIpcResponse::Error(_) => "Error",
        SearchPlaneQueryIpcResponse::Sourcegraph(_) => "Sourcegraph",
        SearchPlaneQueryIpcResponse::RuntimeMetadata(_) => "RuntimeMetadata",
    }
}

fn command_kind_name(kind: CommandKind) -> &'static str {
    match kind {
        CommandKind::Lexical => "lexical",
        CommandKind::Sourcegraph => "sourcegraph",
        CommandKind::Semantic => "semantic",
        CommandKind::Hybrid => "hybrid",
        CommandKind::Explain => "explain",
        CommandKind::RepoMap => "repomap",
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

fn usage() -> &'static str {
    "\
quanta-index-searchctl

Global flags:
  --socket PATH
  --state-root PATH
  --output pretty|json

Read-only subcommands:
  lexical  --repo-id ID --revision-id REV --manifest-generation N --syntax native|sourcegraph --query-text TEXT
  sourcegraph --repo-id ID --revision-id REV --manifest-generation N --query-text TEXT --sg-version SG-X.Y.Z --top-k N
  semantic --repo-id ID --revision-id REV --manifest-generation N (--query-text TEXT | --query-vector CSV|JSON | --query-vector-handle ID) --top-k N [--scope-query TEXT --scope-syntax native|sourcegraph --scope-top-k N]
  hybrid   --repo-id ID --revision-id REV --manifest-generation N --lexical-query TEXT --lexical-syntax native|sourcegraph (--semantic-query TEXT | --semantic-vector CSV|JSON | --semantic-vector-handle ID) --top-k N
  explain  --repo-id ID --revision-id REV --manifest-generation N --candidate-json PATH|-
  repomap  --repo-id ID --revision-id REV --manifest-generation N --query-text TEXT --top-k N --token-budget N [--focus-subject subject_identity:subject_doc_type]
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
    #[expect(
        clippy::panic,
        reason = "test asserts payload variant shape; panic isolates failure to this single test"
    )]
    fn parses_semantic_query_vector_and_backfills_query_text() {
        let parsed = ParsedCommand::parse([
            "semantic",
            "--repo-id",
            "repo",
            "--revision-id",
            "rev",
            "--manifest-generation",
            "7",
            "--query-vector",
            "1.0,0.0,2.5",
            "--top-k",
            "5",
        ]);
        assert!(parsed.is_ok());
        let Ok(parsed) = parsed else {
            return;
        };
        let SearchPlaneQueryIpcRequest::Semantic(request) = parsed.request.payload else {
            panic!("expected semantic payload");
        };
        assert_eq!(request.query_text, Some("1 0 2.5".to_string()));
        assert_eq!(request.query_vector, None);
        assert_eq!(
            request.query_vector_ref,
            Some(SemanticVectorRef::Inline(vec![1.0, 0.0, 2.5]))
        );
    }

    #[test]
    #[expect(
        clippy::panic,
        reason = "test asserts payload variant shape; panic isolates failure to this single test"
    )]
    fn parses_hybrid_semantic_vector_and_backfills_semantic_query() {
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
            "--semantic-vector",
            "[1.0, 0.0, 2.5]",
            "--top-k",
            "5",
        ]);
        assert!(parsed.is_ok());
        let Ok(parsed) = parsed else {
            return;
        };
        let SearchPlaneQueryIpcRequest::Hybrid(request) = parsed.request.payload else {
            panic!("expected hybrid payload");
        };
        assert_eq!(request.semantic_query_text, Some("1 0 2.5".to_string()));
        assert_eq!(request.semantic_vector, None);
        assert_eq!(
            request.semantic_vector_ref,
            Some(SemanticVectorRef::Inline(vec![1.0, 0.0, 2.5]))
        );
    }

    #[test]
    fn rejects_semantic_text_and_vector_together() {
        let parsed = ParsedCommand::parse([
            "semantic",
            "--repo-id",
            "repo",
            "--revision-id",
            "rev",
            "--manifest-generation",
            "7",
            "--query-text",
            "1 2 3",
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
        assert!(error.message.contains("--query-text"));
        assert!(error.message.contains("--query-vector"));
    }

    #[test]
    #[expect(
        clippy::panic,
        reason = "test asserts payload variant shape; panic isolates failure to this single test"
    )]
    fn parses_semantic_query_handle_and_backfills_empty_query_text() {
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
        assert!(parsed.is_ok());
        let Ok(parsed) = parsed else {
            return;
        };
        let SearchPlaneQueryIpcRequest::Semantic(request) = parsed.request.payload else {
            panic!("expected semantic payload");
        };
        assert_eq!(request.query_text, None);
        assert_eq!(request.query_vector, None);
        assert_eq!(
            request.query_vector_ref,
            Some(SemanticVectorRef::Handle("emb-123".into()))
        );
    }

    #[test]
    #[expect(
        clippy::panic,
        reason = "test asserts payload variant shape; panic isolates failure to this single test"
    )]
    fn parses_hybrid_handle_and_backfills_empty_semantic_query() {
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
            "--semantic-vector-handle",
            "emb-456",
            "--top-k",
            "5",
        ]);
        assert!(parsed.is_ok());
        let Ok(parsed) = parsed else {
            return;
        };
        let SearchPlaneQueryIpcRequest::Hybrid(request) = parsed.request.payload else {
            panic!("expected hybrid payload");
        };
        assert_eq!(request.semantic_query_text, None);
        assert_eq!(request.semantic_vector, None);
        assert_eq!(
            request.semantic_vector_ref,
            Some(SemanticVectorRef::Handle("emb-456".into()))
        );
    }

    #[test]
    #[expect(
        clippy::panic,
        reason = "test asserts payload variant shape; panic isolates failure to this single test"
    )]
    fn parses_sourcegraph_query_request() {
        let parsed = ParsedCommand::parse([
            "sourcegraph",
            "--repo-id",
            "repo",
            "--revision-id",
            "rev",
            "--manifest-generation",
            "7",
            "--query-text",
            "repo:repo lang:rust needle",
            "--sg-version",
            "sg-5.5.0",
            "--top-k",
            "11",
        ]);
        assert!(parsed.is_ok());
        let Ok(parsed) = parsed else {
            return;
        };
        let SearchPlaneQueryIpcRequest::Sourcegraph(request) = parsed.request.payload else {
            panic!("expected sourcegraph payload");
        };
        assert_eq!(request.source_syntax.as_ref(), "repo:repo lang:rust needle");
        assert_eq!(request.sg_version.as_ref(), "sg-5.5.0");
        assert_eq!(request.top_k, 11);
        assert_eq!(
            request.generation.map(|pin| pin.manifest_generation.get()),
            Some(7)
        );
    }

    #[test]
    fn pretty_renderer_supports_sourcegraph_response() {
        let response = SearchPlaneQueryIpcResponseEnvelope {
            request_id: 1,
            payload: SearchPlaneQueryIpcResponse::Sourcegraph(
                quanta_index_contract::SearchPlaneSourcegraphQueryResponse {
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
                        repo_relative_path: quanta_index_contract::RepoRelativePath::new(
                            "src/lib.rs",
                        ),
                        start_line: 1,
                        end_line: 3,
                        score: 0.5,
                        snippet: "fn sample() {}".to_string(),
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
            assert!(text.contains("kind: sourcegraph"));
            assert!(text.contains("results: 1"));
        }
    }
}
