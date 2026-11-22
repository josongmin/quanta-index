//! Read and publish Native DSL authorities through the SDK.

use super::*;
use quanta_index_contract::{
    RepoId, RevisionId,
    index::{OperationStatus, SubmitOutcome},
    native_protocol::{NativeDslCursorV1, NativeDslKindV1, NativeDslQueryV1, NativeDslScopeV1},
};
use quanta_index_sdk::native_uds::NativeDslPublicationV1;

pub(super) fn handles(command: &str) -> bool {
    matches!(command, "structural" | "runtime-metadata" | "publish-dsl")
}

fn required<T>(value: Option<T>, name: &str) -> CliResult<T> {
    value.ok_or_else(|| CliError::usage(format!("native DSL requires {name}")))
}

pub(super) fn run(args: &[String], stdout: &mut dyn Write) -> CliResult<ExitCode> {
    let mut rest: VecDeque<String> = args.iter().skip(1).cloned().collect();
    let command = required(args.first().map(String::as_str), "a command")?;
    let mut socket = None;
    let mut session = None;
    let mut repository = None;
    let mut revision = None;
    let mut query = None;
    let mut syntax = None;
    let mut top_k = None;
    let mut after: Option<NativeDslCursorV1> = None;
    let mut input = None;
    let mut output = None;
    while let Some(flag) = rest.pop_front() {
        let raw = value(&mut rest, &flag)?;
        match flag.as_str() {
            "--socket" => set_once(&mut socket, PathBuf::from(raw), &flag)?,
            "--input" if command == "publish-dsl" => {
                set_once(&mut input, PathBuf::from(raw), &flag)?
            }
            "--output" if raw == "json" => set_once(&mut output, raw, &flag)?,
            _ if command == "publish-dsl" => {
                return Err(CliError::usage(format!(
                    "unsupported Native DSL publication flag {flag}"
                )));
            }
            "--session-json" => set_once(
                &mut session,
                serde_json::from_str(&raw)
                    .map_err(|error| CliError::usage(format!("invalid session: {error}")))?,
                &flag,
            )?,
            "--repo" => set_once(
                &mut repository,
                RepoId::new(raw).map_err(|error| CliError::usage(error.to_string()))?,
                &flag,
            )?,
            "--revision" => set_once(
                &mut revision,
                RevisionId::new(raw).map_err(|error| CliError::usage(error.to_string()))?,
                &flag,
            )?,
            "--query-text" => set_once(&mut query, raw, &flag)?,
            "--syntax" => set_once(&mut syntax, text_syntax(&raw)?, &flag)?,
            "--top-k" => set_once(
                &mut top_k,
                raw.parse::<u32>()
                    .map_err(|error| CliError::usage(error.to_string()))?,
                &flag,
            )?,
            "--after-json" => set_once(
                &mut after,
                serde_json::from_str(&raw).map_err(|error| {
                    CliError::usage(format!("invalid DSL continuation: {error}"))
                })?,
                &flag,
            )?,
            _ => {
                return Err(CliError::usage(format!(
                    "unsupported Native DSL flag {flag}"
                )));
            }
        }
    }
    let socket = required(socket, "--socket PATH")?;
    if !socket.is_absolute() {
        return Err(CliError::usage("native --socket must be absolute".into()));
    }
    let client = NativeUdsClient::new(&socket);
    let budget = RequestBudgetV1::for_duration(Duration::from_secs(30));
    let result = if command == "publish-dsl" {
        let path = required(input, "--input JSON")?;
        let file = std::fs::File::open(path)
            .map_err(|error| CliError::usage(format!("open DSL publication: {error}")))?;
        use std::io::Read as _;
        const LIMIT: u64 = 16 * 1024 * 1024;
        let mut bytes = Vec::new();
        let bytes_read = file
            .take(LIMIT + 1)
            .read_to_end(&mut bytes)
            .map_err(|error| CliError::usage(format!("read DSL publication: {error}")))?;
        if bytes_read as u64 > LIMIT {
            return Err(CliError::usage(
                "DSL publication input exceeds 16 MiB".into(),
            ));
        }
        let input: NativeDslPublicationV1 = serde_json::from_slice(&bytes)
            .map_err(|error| CliError::usage(format!("invalid DSL publication: {error}")))?;
        let published = client
            .publish_dsl_v1(&input, &budget)
            .map_err(|error| map_error(&error, &socket))?;
        match published {
            SubmitOutcome::Terminal(OperationStatus::Published(view)) => serde_json::to_value(view),
            SubmitOutcome::Terminal(OperationStatus::Refused(reason)) => {
                return Err(CliError::remote(format!(
                    "Native DSL publication refused: {reason:?}"
                )));
            }
            other => {
                return Err(CliError::remote(format!(
                    "Native DSL publication requires reconciliation: {other:?}"
                )));
            }
        }
    } else {
        let query = NativeDslQueryV1 {
            scope: NativeDslScopeV1 {
                repository: required(repository, "--repo")?,
                revision: required(revision, "--revision")?,
            },
            kind: match command {
                "structural" => NativeDslKindV1::Structural,
                "runtime-metadata" => NativeDslKindV1::RuntimeMetadata,
                _ => return Err(CliError::usage("unknown Native DSL query".into())),
            },
            text: TextQueryRequest {
                syntax: syntax.unwrap_or(TextQuerySyntax::Native),
                query_text: required(query, "--query-text")?,
                constraints: QueryConstraintSetV1::unconstrained(),
                generation: None,
                generation_selector: None,
                top_k: required(top_k, "--top-k")?,
                cursor: None,
            },
            after,
        };
        let _digest = query
            .digest()
            .map_err(|error| CliError::usage(error.into()))?;
        let session = required(session, "--session-json")?;
        serde_json::to_value(
            client
                .search_dsl_in_read_session_v1(&session, query, &budget)
                .map_err(|error| map_error(&error, &socket))?,
        )
    }
    .map_err(|error| CliError::protocol(format!("Native DSL result encoding: {error}")))?;
    let rendered = serde_json::to_string_pretty(&result)
        .map_err(|error| CliError::protocol(format!("Native DSL result encoding: {error}")))?;
    write_stdout(stdout, &rendered)?;
    Ok(ExitCode::SUCCESS)
}
