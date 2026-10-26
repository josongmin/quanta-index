//! Explicit native CLI route. It never opens a legacy `SearchPlane` client.

use std::{
    collections::VecDeque,
    fmt::Write as _,
    io::Write,
    path::{Path, PathBuf},
    process::ExitCode,
    time::Duration,
};

use quanta_index_contract::{
    QueryConstraintSetV1, TextQueryRequest, TextQuerySyntax,
    index::{
        CollectionId, IndexInstanceId, SlotId, TenantId, ViewSelection, WorkspaceKey, WorktreeId,
    },
    native_protocol::MAX_NATIVE_TEXT_TOP_K,
};
use quanta_index_ipc::RequestBudgetV1;
use quanta_index_sdk::native_uds::{NativeUdsClient, NativeUdsError};

use crate::{CliError, CliResult, EXIT_TRANSPORT, write_stdout};

#[derive(Clone, Copy)]
enum NativeCommand {
    ReadView,
    SearchText,
}

#[derive(Clone, Copy)]
enum NativeOutput {
    Pretty,
    Json,
}

struct NativeOptions {
    command: NativeCommand,
    socket: PathBuf,
    workspace: WorkspaceKey,
    epoch: u64,
    instance: Option<IndexInstanceId>,
    query_text: Option<String>,
    top_k: Option<u32>,
    output: NativeOutput,
}

fn value(rest: &mut VecDeque<String>, flag: &str) -> CliResult<String> {
    rest.pop_front()
        .ok_or_else(|| CliError::usage(format!("{flag} requires a value")))
}

fn set_once<T>(slot: &mut Option<T>, next: T, flag: &str) -> CliResult<()> {
    if slot.replace(next).is_some() {
        return Err(CliError::usage(format!(
            "{flag} may be specified only once"
        )));
    }
    Ok(())
}

fn bytes<const N: usize>(text: &str, name: &str) -> CliResult<[u8; N]> {
    let expected = N
        .checked_mul(2)
        .ok_or_else(|| CliError::usage(format!("{name} length overflow")))?;
    if text.len() != expected || !text.is_ascii() {
        return Err(CliError::usage(format!(
            "{name} requires {expected} hex characters"
        )));
    }
    let mut decoded = [0_u8; N];
    for (index, pair) in text.as_bytes().chunks_exact(2).enumerate() {
        let string = std::str::from_utf8(pair)
            .map_err(|error| CliError::usage(format!("{name} is not UTF-8: {error}")))?;
        let byte = u8::from_str_radix(string, 16)
            .map_err(|error| CliError::usage(format!("{name} has invalid hex: {error}")))?;
        let entry = decoded
            .get_mut(index)
            .ok_or_else(|| CliError::usage(format!("{name} exceeds its identity length")))?;
        *entry = byte;
    }
    Ok(decoded)
}

fn workspace(text: &str) -> CliResult<WorkspaceKey> {
    let raw = bytes::<64>(text, "workspace")?;
    let [tenant, collection, worktree, slot] = raw.as_chunks::<16>().0 else {
        return Err(CliError::usage("workspace must have four IDs".into()));
    };
    Ok(WorkspaceKey {
        tenant: TenantId(*tenant),
        collection: CollectionId(*collection),
        worktree: WorktreeId(*worktree),
        slot: SlotId(*slot),
    })
}

fn parse(args: &[String]) -> CliResult<NativeOptions> {
    let mut rest: VecDeque<String> = args.iter().cloned().collect();
    let command = match rest.pop_front().as_deref() {
        Some("read-view") => NativeCommand::ReadView,
        Some("search-text") => NativeCommand::SearchText,
        Some(other) => {
            return Err(CliError::usage(format!(
                "unsupported native command {other}"
            )));
        }
        None => {
            return Err(CliError::usage(
                "native needs read-view or search-text".into(),
            ));
        }
    };
    let mut socket = None;
    let mut workspace = None;
    let mut epoch = None;
    let mut instance = None;
    let mut query_text = None;
    let mut top_k = None;
    let mut output = NativeOutput::Pretty;
    let mut output_seen = false;
    while let Some(flag) = rest.pop_front() {
        match flag.as_str() {
            "--socket" => set_once(&mut socket, PathBuf::from(value(&mut rest, &flag)?), &flag)?,
            "--workspace" => set_once(
                &mut workspace,
                self::workspace(&value(&mut rest, &flag)?)?,
                &flag,
            )?,
            "--epoch" => {
                let raw = value(&mut rest, &flag)?;
                let parsed = raw
                    .parse::<u64>()
                    .map_err(|error| CliError::usage(format!("invalid native epoch: {error}")))?;
                set_once(&mut epoch, parsed, &flag)?;
            }
            "--instance" => set_once(
                &mut instance,
                IndexInstanceId(bytes::<16>(&value(&mut rest, &flag)?, "instance")?),
                &flag,
            )?,
            "--query-text" => set_once(&mut query_text, value(&mut rest, &flag)?, &flag)?,
            "--top-k" => {
                let raw = value(&mut rest, &flag)?;
                let parsed = raw
                    .parse::<u32>()
                    .map_err(|error| CliError::usage(format!("invalid native top-k: {error}")))?;
                set_once(&mut top_k, parsed, &flag)?;
            }
            "--output" => {
                if output_seen {
                    return Err(CliError::usage(
                        "--output may be specified only once".into(),
                    ));
                }
                output_seen = true;
                output = match value(&mut rest, &flag)?.as_str() {
                    "pretty" => NativeOutput::Pretty,
                    "json" => NativeOutput::Json,
                    other => {
                        return Err(CliError::usage(format!(
                            "native output {other} is unsupported"
                        )));
                    }
                };
            }
            other => return Err(CliError::usage(format!("unknown native argument {other}"))),
        }
    }
    let socket: PathBuf =
        socket.ok_or_else(|| CliError::usage("native requires --socket PATH".into()))?;
    if !socket.is_absolute() {
        return Err(CliError::usage("native --socket must be absolute".into()));
    }
    let workspace =
        workspace.ok_or_else(|| CliError::usage("native requires --workspace HEX128".into()))?;
    let epoch = epoch.ok_or_else(|| CliError::usage("native requires --epoch N".into()))?;
    match command {
        NativeCommand::ReadView
            if instance.is_some() || query_text.is_some() || top_k.is_some() =>
        {
            return Err(CliError::usage(
                "native read-view does not accept query flags".into(),
            ));
        }
        NativeCommand::SearchText
            if instance.is_none() || query_text.is_none() || top_k.is_none() =>
        {
            return Err(CliError::usage(
                "native search-text needs --instance, --query-text and --top-k".into(),
            ));
        }
        NativeCommand::SearchText
            if !top_k.is_some_and(|limit| (1..=MAX_NATIVE_TEXT_TOP_K).contains(&limit)) =>
        {
            return Err(CliError::usage(format!(
                "native --top-k must be 1..={MAX_NATIVE_TEXT_TOP_K}"
            )));
        }
        NativeCommand::ReadView | NativeCommand::SearchText => {}
    }
    Ok(NativeOptions {
        command,
        socket,
        workspace,
        epoch,
        instance,
        query_text,
        top_k,
        output,
    })
}

fn map_error(error: &NativeUdsError, socket: &Path) -> CliError {
    let detail = format!("native socket {}: {error}", socket.display());
    match error {
        NativeUdsError::Transport(_) => CliError::transport(detail),
        NativeUdsError::Protocol(_) => CliError::protocol(detail),
        NativeUdsError::Refused(_) => CliError::remote(detail),
        NativeUdsError::Unknown(_) => CliError {
            exit_code: EXIT_TRANSPORT,
            message: detail,
        },
    }
}

pub(super) fn run(args: &[String], stdout: &mut dyn Write) -> CliResult<ExitCode> {
    let parsed = parse(args)?;
    let client = NativeUdsClient::new(parsed.socket.clone());
    let selection = ViewSelection::Latest {
        workspace: parsed.workspace,
        epoch: parsed.epoch,
    };
    let budget = RequestBudgetV1::for_duration(Duration::from_secs(30));
    match parsed.command {
        NativeCommand::ReadView => {
            let view = client
                .read_view(selection, &budget)
                .map_err(|error| map_error(&error, &parsed.socket))?;
            let rendered = match parsed.output {
                NativeOutput::Json => serde_json::to_string_pretty(&view).map_err(|error| {
                    CliError::protocol(format!("native view encoding: {error}"))
                })?,
                NativeOutput::Pretty => {
                    format!("view: {:?}\nsource: {:?}\n", view.identity, view.source)
                }
            };
            write_stdout(stdout, &rendered)?;
        }
        NativeCommand::SearchText => {
            let instance = parsed
                .instance
                .ok_or_else(|| CliError::usage("native instance missing".into()))?;
            let query_text = parsed
                .query_text
                .ok_or_else(|| CliError::usage("native query text missing".into()))?;
            let top_k = parsed
                .top_k
                .ok_or_else(|| CliError::usage("native top-k missing".into()))?;
            let request = TextQueryRequest {
                syntax: TextQuerySyntax::Native,
                query_text,
                constraints: QueryConstraintSetV1::unconstrained(),
                generation: None,
                generation_selector: None,
                top_k,
                cursor: None,
            };
            let ranked = client
                .search_text(selection, instance, request, &budget)
                .map_err(|error| map_error(&error, &parsed.socket))?;
            let rendered = match parsed.output {
                NativeOutput::Json => serde_json::to_string_pretty(&ranked).map_err(|error| {
                    CliError::protocol(format!("native text encoding: {error}"))
                })?,
                NativeOutput::Pretty => {
                    let mut value = format!(
                        "view: {:?}\nmatched: {}\nexhaustive: {}\n",
                        ranked.view, ranked.matched, ranked.exhaustive
                    );
                    for hit in &ranked.hits {
                        writeln!(value, "{} {:?} {:?}", hit.path, hit.owner, hit.score).map_err(
                            |error| CliError::protocol(format!("native text rendering: {error}")),
                        )?;
                    }
                    value
                }
            };
            write_stdout(stdout, &rendered)?;
        }
    }
    Ok(ExitCode::SUCCESS)
}
