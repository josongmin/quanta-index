//! History rows and pages through an explicit existing Native read session.

use super::{
    CliError, CliResult, Duration, ExitCode, IndexInstanceId, NativeUdsClient, PathBuf,
    QueryConstraintSetV1, RequestBudgetV1, TextQueryRequest, TextQuerySyntax, VecDeque, Write,
    bytes, map_error, set_once, value, write_stdout,
};
use quanta_index_contract::{
    HistoryOrderV1, HistoryQueryRequest,
    native_protocol::{NativeHistoryCursor, NativeReadSessionStatus},
};

fn required<T>(value: Option<T>, name: &str) -> CliResult<T> {
    value.ok_or_else(|| CliError::usage(format!("native history requires {name}")))
}

pub(super) fn run(args: &[String], stdout: &mut dyn Write) -> CliResult<ExitCode> {
    let mut rest: VecDeque<String> = args.iter().skip(1).cloned().collect();
    let mut socket = None;
    let mut status: Option<NativeReadSessionStatus> = None;
    let mut instance = None;
    let mut text = None;
    let mut top_k = None;
    let mut order = None;
    let mut syntax = None;
    let mut after: Option<NativeHistoryCursor> = None;
    let mut output = None;
    while let Some(flag) = rest.pop_front() {
        let raw = value(&mut rest, &flag)?;
        match flag.as_str() {
            "--socket" => set_once(&mut socket, PathBuf::from(raw), &flag)?,
            "--session-json" => set_once(
                &mut status,
                serde_json::from_str(&raw)
                    .map_err(|error| CliError::usage(format!("invalid session: {error}")))?,
                &flag,
            )?,
            "--instance" => set_once(
                &mut instance,
                IndexInstanceId(bytes::<16>(&raw, "History instance")?),
                &flag,
            )?,
            "--query-text" => set_once(&mut text, raw, &flag)?,
            "--top-k" => set_once(
                &mut top_k,
                raw.parse::<u32>()
                    .map_err(|error| CliError::usage(format!("invalid top-k: {error}")))?,
                &flag,
            )?,
            "--order" => set_once(
                &mut order,
                HistoryOrderV1::from_code_str(&raw).ok_or_else(|| {
                    CliError::usage("History order requires recency or relevance".into())
                })?,
                &flag,
            )?,
            "--syntax" => set_once(
                &mut syntax,
                match raw.as_str() {
                    "native" => TextQuerySyntax::Native,
                    "sourcegraph" => TextQuerySyntax::Sourcegraph,
                    _ => {
                        return Err(CliError::usage(
                            "History syntax requires native or sourcegraph".into(),
                        ));
                    }
                },
                &flag,
            )?,
            "--after-json" => set_once(
                &mut after,
                serde_json::from_str(&raw).map_err(|error| {
                    CliError::usage(format!("invalid History continuation: {error}"))
                })?,
                &flag,
            )?,
            "--output" if raw == "json" => set_once(&mut output, raw, &flag)?,
            "--output" => {
                return Err(CliError::usage(
                    "retained History supports --output json only".into(),
                ));
            }
            _ => {
                return Err(CliError::usage(format!(
                    "unsupported Native History argument {flag}"
                )));
            }
        }
    }
    let socket = required(socket, "--socket PATH")?;
    if !socket.is_absolute() {
        return Err(CliError::usage("native --socket must be absolute".into()));
    }
    let status = required(status, "--session-json JSON")?;
    let query = HistoryQueryRequest {
        text_query: TextQueryRequest {
            syntax: syntax.unwrap_or(TextQuerySyntax::Native),
            query_text: required(text, "--query-text TEXT")?,
            constraints: QueryConstraintSetV1::unconstrained(),
            generation: None,
            generation_selector: None,
            cursor: None,
            top_k: required(top_k, "--top-k N")?,
        },
        order: required(order, "--order recency|relevance")?,
        cursor: None,
    };
    let instance = required(instance, "--instance HEX32")?;
    let budget = RequestBudgetV1::for_duration(Duration::from_secs(30));
    let client = NativeUdsClient::new(&socket);
    let page = client
        .search_history_in_read_session(&status, instance, query, after, &budget)
        .map_err(|error| map_error(&error, &socket))?;
    let rendered = serde_json::to_string_pretty(&page)
        .map_err(|error| CliError::protocol(format!("History result encoding: {error}")))?;
    write_stdout(stdout, &rendered)?;
    Ok(ExitCode::SUCCESS)
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use quanta_index_contract::{
        AuxEpochV1, DiffCandidate, DiffHunkSide, QueryResultWindowV2,
        index::{Digest, ObjectRef},
        native_protocol::{
            NativeHistoryPage, NativeIndexRequest, NativeIndexRequestEnvelope, NativeIndexResponse,
            NativeIndexResponseEnvelope, NativeReadSessionRequest,
        },
    };
    use std::{os::unix::net::UnixListener, time::Instant};
    type TestResult = Result<(), Box<dyn std::error::Error>>;
    fn status() -> Result<NativeReadSessionStatus, serde_json::Error> {
        serde_json::from_value(serde_json::json!({"session":vec![7_u8;32],"view":{
            "workspace":{"tenant":vec![1_u8;16],"collection":vec![2_u8;16],"worktree":vec![3_u8;16],"slot":vec![4_u8;16]},
            "epoch":1,"sequence":9,"digest":vec![5_u8;32]},"expires_at_ms":100,"revision":1,"state":"Active"}))
    }
    #[test]
    fn native_history_cli_preserves_diff_rows_filters_and_each_order() -> TestResult {
        for order in [HistoryOrderV1::Recency, HistoryOrderV1::Relevance] {
            let dir = tempfile::tempdir()?;
            let socket = dir.path().join("history.sock");
            let listener = UnixListener::bind(&socket)?;
            listener.set_nonblocking(true)?;
            let status = status()?;
            let instance = IndexInstanceId([8; 16]);
            let expected = NativeHistoryPage {
                session: status.session,
                view: status.view,
                instance,
                member: ObjectRef(Digest([9; 32])),
                order,
                commits: Vec::new(),
                diffs: vec![DiffCandidate {
                    repo_relative_path: "deleted/old.rs".into(),
                    hunk_header: "@@ -1 +0 @@".into(),
                    side: DiffHunkSide::Before,
                    line_start: 1,
                    line_end: 1,
                    snippet: "historic fix".into(),
                    score: match order {
                        HistoryOrderV1::Recency => None,
                        HistoryOrderV1::Relevance => {
                            Some(quanta_index_contract::HistoryScoreV1::try_new(0.75)?)
                        }
                    },
                }],
                matched: 1,
                remaining: 1,
                window: QueryResultWindowV2::exact_probe(1),
                read_epoch: AuxEpochV1::new(3),
                examined: 17,
                after: None,
            };
            let sent = expected.clone();
            let peer = std::thread::spawn(move || -> Result<(), String> {
                let deadline = Instant::now() + Duration::from_secs(5);
                let mut stream = loop {
                    match listener.accept() {
                        Ok((stream, _)) => break stream,
                        Err(error)
                            if error.kind() == std::io::ErrorKind::WouldBlock
                                && Instant::now() < deadline =>
                        {
                            std::thread::sleep(Duration::from_millis(2))
                        }
                        Err(error) => return Err(error.to_string()),
                    }
                };
                stream
                    .set_nonblocking(false)
                    .map_err(|error| error.to_string())?;
                stream
                    .set_read_timeout(Some(Duration::from_secs(5)))
                    .map_err(|error| error.to_string())?;
                let request: NativeIndexRequestEnvelope =
                    quanta_index_ipc::decode_request(&mut stream)
                        .map_err(|error| error.to_string())?;
                let NativeIndexRequest::ReadSession(NativeReadSessionRequest::SearchHistory {
                    session,
                    instance: actual,
                    query,
                    after,
                }) = request.payload
                else {
                    return Err("CLI did not query History on the supplied retained session".into());
                };
                if session != sent.session
                    || actual != instance
                    || query.order != order
                    || after.is_some()
                    || query.text_query.query_text != "type:diff fix file:deleted/"
                    || query.text_query.top_k != 2
                {
                    return Err("CLI changed History predicate/order/session/instance/cap".into());
                }
                let response = NativeIndexResponseEnvelope {
                    request_id: request.request_id,
                    payload: NativeIndexResponse::HistoryPage(sent),
                };
                let frame = quanta_index_ipc::encode_response(&response)
                    .map_err(|error| error.to_string())?;
                stream.write_all(&frame).map_err(|error| error.to_string())
            });
            let mut output = Vec::new();
            let mut errors = Vec::new();
            let exit = crate::run(
                vec![
                    "history".to_string(),
                    "--socket".into(),
                    socket.to_string_lossy().into_owned(),
                    "--session-json".into(),
                    serde_json::to_string(&status)?,
                    "--instance".into(),
                    "08".repeat(16),
                    "--query-text".into(),
                    "type:diff fix file:deleted/".into(),
                    "--top-k".into(),
                    "2".into(),
                    "--order".into(),
                    order.as_code_str().into(),
                    "--output".into(),
                    "json".into(),
                ],
                &mut output,
                &mut errors,
            );
            peer.join().map_err(|_| "History CLI peer panicked")??;
            assert_eq!(
                exit,
                ExitCode::SUCCESS,
                "{}",
                String::from_utf8_lossy(&errors)
            );
            let actual: NativeHistoryPage = serde_json::from_slice(&output)?;
            assert_eq!(actual, expected);
        }
        Ok(())
    }
    #[test]
    fn native_history_cli_refuses_pretty_and_missing_order_before_connect() -> TestResult {
        let dir = tempfile::tempdir()?;
        let socket = dir.path().join("no-connect.sock");
        let listener = UnixListener::bind(&socket)?;
        listener.set_nonblocking(true)?;
        let args = vec![
            "history".to_string(),
            "--socket".into(),
            socket.to_string_lossy().into_owned(),
            "--session-json".into(),
            serde_json::to_string(&status()?)?,
            "--instance".into(),
            "08".repeat(16),
            "--query-text".into(),
            "type:commit fix".into(),
            "--top-k".into(),
            "2".into(),
        ];
        let mut output = Vec::new();
        let mut errors = Vec::new();
        assert_eq!(
            crate::run(args.clone(), &mut output, &mut errors),
            ExitCode::from(2)
        );
        assert!(String::from_utf8_lossy(&errors).contains("--order"));
        let mut pretty = args;
        pretty.extend([
            "--order".into(),
            "recency".into(),
            "--output".into(),
            "pretty".into(),
        ]);
        errors.clear();
        assert_eq!(
            crate::run(pretty, &mut output, &mut errors),
            ExitCode::from(2)
        );
        assert!(String::from_utf8_lossy(&errors).contains("json only"));
        assert!(
            matches!(listener.accept(),Err(error) if error.kind()==std::io::ErrorKind::WouldBlock)
        );
        Ok(())
    }
}
