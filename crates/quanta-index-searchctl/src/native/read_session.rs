//! Explicit retained Native Query commands; lifecycle is caller controlled.

use super::*;
use quanta_index_contract::{
    SemanticQueryRequest,
    native_protocol::{
        MAX_NATIVE_SEMANTIC_TOP_K, NativeReadSessionStatus, NativeTextCursor,
        NativeTextPathCursorV1,
    },
};

pub(super) fn handles(command: &str) -> bool {
    matches!(
        command,
        "read-session-open"
            | "read-session-renew"
            | "read-session-status"
            | "read-session-profile"
            | "read-session-release"
            | "read-session-settlement"
            | "search-text-page"
            | "search-text-path-page"
            | "search-hybrid-file"
    )
}

fn required<T>(value: Option<T>, flag: &str) -> CliResult<T> {
    value.ok_or_else(|| CliError::usage(format!("native command requires {flag}")))
}

pub(super) fn run(args: &[String], stdout: &mut dyn Write) -> CliResult<ExitCode> {
    let mut rest: VecDeque<String> = args.iter().cloned().collect();
    let command = required(rest.pop_front(), "command")?;
    let mut socket = None;
    let mut selection_workspace = None;
    let mut epoch = None;
    let mut session: Option<NativeReadSessionStatus> = None;
    let mut open_key = None;
    let mut instance = None;
    let mut semantic_instance = None;
    let mut query_text = None;
    let mut syntax = None;
    let mut semantic_text = None;
    let mut top_k = None;
    let mut file_top_k = None;
    let mut after: Option<NativeTextCursor> = None;
    let mut after_path: Option<NativeTextPathCursorV1> = None;
    let mut path_exact = None;
    let mut output = None;
    while let Some(flag) = rest.pop_front() {
        let raw = value(&mut rest, &flag)?;
        match flag.as_str() {
            "--socket" => set_once(&mut socket, PathBuf::from(raw), &flag)?,
            "--workspace" => set_once(&mut selection_workspace, workspace(&raw)?, &flag)?,
            "--epoch" => set_once(
                &mut epoch,
                raw.parse::<u64>()
                    .map_err(|error| CliError::usage(error.to_string()))?,
                &flag,
            )?,
            "--session-json" => set_once(
                &mut session,
                serde_json::from_str(&raw)
                    .map_err(|error| CliError::usage(format!("invalid {flag}: {error}")))?,
                &flag,
            )?,
            "--open-key" => set_once(&mut open_key, bytes::<32>(&raw, "open key")?, &flag)?,
            "--instance" => set_once(
                &mut instance,
                IndexInstanceId(bytes::<16>(&raw, "instance")?),
                &flag,
            )?,
            "--semantic-instance" => set_once(
                &mut semantic_instance,
                IndexInstanceId(bytes::<16>(&raw, "semantic instance")?),
                &flag,
            )?,
            "--query-text" => set_once(&mut query_text, raw, &flag)?,
            "--syntax"
                if matches!(
                    command.as_str(),
                    "search-text-page" | "search-text-path-page"
                ) =>
            {
                set_once(&mut syntax, text_syntax(&raw)?, &flag)?;
            }
            "--semantic-query-text" => set_once(&mut semantic_text, raw, &flag)?,
            "--top-k" => set_once(
                &mut top_k,
                raw.parse::<u32>()
                    .map_err(|error| CliError::usage(error.to_string()))?,
                &flag,
            )?,
            "--file-top-k" => set_once(
                &mut file_top_k,
                raw.parse::<u32>()
                    .map_err(|error| CliError::usage(error.to_string()))?,
                &flag,
            )?,
            "--after-json" if command == "search-text-path-page" => set_once(
                &mut after_path,
                serde_json::from_str(&raw)
                    .map_err(|error| CliError::usage(format!("invalid {flag}: {error}")))?,
                &flag,
            )?,
            "--after-json" => set_once(
                &mut after,
                serde_json::from_str(&raw)
                    .map_err(|error| CliError::usage(format!("invalid {flag}: {error}")))?,
                &flag,
            )?,
            "--path-exact" => set_once(&mut path_exact, raw, &flag)?,
            "--output" if raw == "json" => set_once(&mut output, raw, &flag)?,
            "--output" => {
                return Err(CliError::usage(
                    "retained native commands support --output json only".into(),
                ));
            }
            _ => {
                return Err(CliError::usage(format!(
                    "unsupported native argument {flag}"
                )));
            }
        }
    }
    let socket = required(socket, "--socket PATH")?;
    if !socket.is_absolute() {
        return Err(CliError::usage("native --socket must be absolute".into()));
    }
    let lifecycle = command.starts_with("read-session-");
    if lifecycle
        && (instance.is_some()
            || semantic_instance.is_some()
            || query_text.is_some()
            || semantic_text.is_some()
            || top_k.is_some()
            || file_top_k.is_some()
            || after.is_some()
            || after_path.is_some()
            || path_exact.is_some())
    {
        return Err(CliError::usage(
            "read-session lifecycle rejects query flags".into(),
        ));
    }
    let budget = RequestBudgetV1::for_duration(Duration::from_secs(30));
    let client = NativeUdsClient::new(&socket);
    let result = if command == "read-session-open" {
        if session.is_some() {
            return Err(CliError::usage("open rejects --session-json".into()));
        }
        let selection = ViewSelection::Latest {
            workspace: required(selection_workspace, "--workspace")?,
            epoch: required(epoch, "--epoch")?,
        };
        let key = required(open_key, "--open-key HEX64")?;
        serde_json::to_value(
            client
                .open_read_session(selection, key, &budget)
                .map_err(|error| map_error(&error, &socket))?,
        )
    } else {
        if open_key.is_some() || selection_workspace.is_some() || epoch.is_some() {
            return Err(CliError::usage(
                "retained commands use --session-json rather than a new selection".into(),
            ));
        }
        let session = required(session, "--session-json JSON")?;
        match command.as_str() {
            "read-session-renew" => serde_json::to_value(
                client
                    .renew_read_session(&session, &budget)
                    .map_err(|error| map_error(&error, &socket))?,
            ),
            "read-session-status" => serde_json::to_value(
                client
                    .read_session_status(&session, &budget)
                    .map_err(|error| map_error(&error, &socket))?,
            ),
            "read-session-profile" => serde_json::to_value(
                client
                    .read_session_profile(&session, &budget)
                    .map_err(|error| map_error(&error, &socket))?,
            ),
            "read-session-release" => {
                let (status, pins) = client
                    .release_read_session(&session, &budget)
                    .map_err(|error| map_error(&error, &socket))?;
                Ok(serde_json::json!({"status":status,"has_remaining_target_pins":pins}))
            }
            "read-session-settlement" => serde_json::to_value(
                client
                    .read_session_settlement(&session, &budget)
                    .map_err(|error| map_error(&error, &socket))?,
            ),
            "search-text-page" | "search-text-path-page" | "search-hybrid-file" => {
                let top_k = required(top_k, "--top-k")?;
                if !(1..=MAX_NATIVE_TEXT_TOP_K).contains(&top_k)
                    || top_k > MAX_NATIVE_SEMANTIC_TOP_K
                {
                    return Err(CliError::usage("native query top-k exceeds bounds".into()));
                }
                let mut constraints = QueryConstraintSetV1::unconstrained();
                if let Some(path) = path_exact {
                    constraints.repo_relative_path_exact = Some(
                        serde_json::from_value(serde_json::Value::String(path)).map_err(
                            |error| CliError::usage(format!("invalid --path-exact: {error}")),
                        )?,
                    );
                }
                let text = TextQueryRequest {
                    syntax: syntax.unwrap_or(TextQuerySyntax::Native),
                    query_text: required(query_text, "--query-text")?,
                    constraints: constraints.clone(),
                    generation: None,
                    generation_selector: None,
                    top_k,
                    cursor: None,
                };
                let instance = required(instance, "--instance")?;
                if matches!(
                    command.as_str(),
                    "search-text-page" | "search-text-path-page"
                ) {
                    if semantic_instance.is_some()
                        || semantic_text.is_some()
                        || file_top_k.is_some()
                    {
                        return Err(CliError::usage("text page rejects semantic flags".into()));
                    }
                    if command == "search-text-path-page" {
                        serde_json::to_value(
                            client
                                .search_text_path_page_in_read_session(
                                    &session, instance, text, after_path, &budget,
                                )
                                .map_err(|error| map_error(&error, &socket))?,
                        )
                    } else {
                        serde_json::to_value(
                            client
                                .search_text_page_in_read_session(
                                    &session, instance, text, after, &budget,
                                )
                                .map_err(|error| map_error(&error, &socket))?,
                        )
                    }
                } else {
                    if after.is_some() {
                        return Err(CliError::usage(
                            "bounded Hybrid has no global continuation".into(),
                        ));
                    }
                    let semantic = SemanticQueryRequest {
                        query_text: required(semantic_text, "--semantic-query-text")?,
                        constraints,
                        generation: None,
                        generation_selector: None,
                        lexical_scope: None,
                        top_k,
                    };
                    let semantic_instance = required(semantic_instance, "--semantic-instance")?;
                    let file_top_k = required(file_top_k, "--file-top-k")?;
                    quanta_index_contract::native_protocol::validate_native_hybrid_file_query(
                        &text, &semantic, file_top_k,
                    )
                    .map_err(CliError::usage)?;
                    serde_json::to_value(
                        client
                            .search_hybrid_file_in_read_session(
                                &session,
                                instance,
                                semantic_instance,
                                text,
                                semantic,
                                file_top_k,
                                &budget,
                            )
                            .map_err(|error| map_error(&error, &socket))?,
                    )
                }
            }
            _ => {
                return Err(CliError::usage(
                    "unsupported retained native command".into(),
                ));
            }
        }
    }
    .map_err(|error| CliError::protocol(format!("native result encoding: {error}")))?;
    let rendered = serde_json::to_string_pretty(&result)
        .map_err(|error| CliError::protocol(error.to_string()))?;
    write_stdout(stdout, &rendered)?;
    Ok(ExitCode::SUCCESS)
}
