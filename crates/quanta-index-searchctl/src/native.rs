//! Explicit native CLI route. It never opens a legacy `SearchPlane` client.

mod history;
mod read_session;

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
    native_protocol::{MAX_NATIVE_TEXT_TOP_K, NativeRepositoryPathCursor},
};
use quanta_index_ipc::RequestBudgetV1;
use quanta_index_sdk::native_uds::{NativeUdsClient, NativeUdsError};

use crate::{CliError, CliResult, EXIT_TRANSPORT, write_stdout};

#[derive(Clone, Copy)]
enum NativeCommand {
    ReadView,
    SearchText,
    QueryRepositoryPaths,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
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
    syntax: TextQuerySyntax,
    top_k: Option<u32>,
    prefix: Option<String>,
    after: Option<NativeRepositoryPathCursor>,
    limit: Option<u32>,
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

fn text_syntax(raw: &str) -> CliResult<TextQuerySyntax> {
    match raw {
        "native" => Ok(TextQuerySyntax::Native),
        "sourcegraph" => Ok(TextQuerySyntax::Sourcegraph),
        other => Err(CliError::usage(format!(
            "unsupported native text syntax {other}"
        ))),
    }
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
        Some("query-repository-paths") => NativeCommand::QueryRepositoryPaths,
        Some("lexical") => {
            return Err(CliError::unsupported(
                quanta_index_sdk::LegacyQueryCapabilityV1::Text,
            ));
        }
        Some("symbol") => {
            return Err(CliError::unsupported(
                quanta_index_sdk::LegacyQueryCapabilityV1::Symbol,
            ));
        }
        Some("semantic") => {
            return Err(CliError::unsupported(
                quanta_index_sdk::LegacyQueryCapabilityV1::Semantic,
            ));
        }
        Some("hybrid") => {
            return Err(CliError::unsupported(
                quanta_index_sdk::LegacyQueryCapabilityV1::Hybrid,
            ));
        }
        Some("go-test-trace-file") => {
            return Err(CliError::unsupported(
                quanta_index_sdk::LegacyQueryCapabilityV1::GoTestTraceFileV1,
            ));
        }
        Some("hybrid-seed") => {
            return Err(CliError::unsupported(
                quanta_index_sdk::LegacyQueryCapabilityV1::HybridSeed,
            ));
        }
        Some("explain") => {
            return Err(CliError::unsupported(
                quanta_index_sdk::LegacyQueryCapabilityV1::Explain,
            ));
        }
        Some("repomap") => {
            return Err(CliError::unsupported(
                quanta_index_sdk::LegacyQueryCapabilityV1::RepoMapQuery,
            ));
        }
        Some("repomap-query") => {
            return Err(CliError::unsupported(
                quanta_index_sdk::LegacyQueryCapabilityV1::RepoMapQuery,
            ));
        }
        Some("runtime-metadata") => {
            return Err(CliError::unsupported(
                quanta_index_sdk::LegacyQueryCapabilityV1::RuntimeMetadata,
            ));
        }
        Some("structural") => {
            return Err(CliError::unsupported(
                quanta_index_sdk::LegacyQueryCapabilityV1::Structural,
            ));
        }
        Some(other) => {
            return Err(CliError::usage(format!(
                "unsupported native command {other}"
            )));
        }
        None => {
            return Err(CliError::usage(
                "native needs read-view, search-text or query-repository-paths".into(),
            ));
        }
    };
    let mut socket = None;
    let mut workspace = None;
    let mut epoch = None;
    let mut instance = None;
    let mut query_text = None;
    let mut syntax = None;
    let mut top_k = None;
    let mut prefix = None;
    let mut after = None;
    let mut limit = None;
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
            "--syntax" => set_once(&mut syntax, text_syntax(&value(&mut rest, &flag)?)?, &flag)?,
            "--top-k" => {
                let raw = value(&mut rest, &flag)?;
                let parsed = raw
                    .parse::<u32>()
                    .map_err(|error| CliError::usage(format!("invalid native top-k: {error}")))?;
                set_once(&mut top_k, parsed, &flag)?;
            }
            "--prefix" if matches!(command, NativeCommand::QueryRepositoryPaths) => {
                set_once(&mut prefix, value(&mut rest, &flag)?, &flag)?;
            }
            "--after-json" if matches!(command, NativeCommand::QueryRepositoryPaths) => {
                let raw = value(&mut rest, &flag)?;
                let cursor = serde_json::from_str(&raw)
                    .map_err(|error| CliError::usage(format!("invalid {flag}: {error}")))?;
                set_once(&mut after, cursor, &flag)?;
            }
            "--limit" if matches!(command, NativeCommand::QueryRepositoryPaths) => {
                let raw = value(&mut rest, &flag)?;
                let parsed = raw
                    .parse::<u32>()
                    .map_err(|error| CliError::usage(format!("invalid native limit: {error}")))?;
                set_once(&mut limit, parsed, &flag)?;
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
    if matches!(command, NativeCommand::QueryRepositoryPaths) && !output_seen {
        output = NativeOutput::Json;
    }
    match command {
        NativeCommand::ReadView
            if instance.is_some()
                || query_text.is_some()
                || syntax.is_some()
                || top_k.is_some()
                || prefix.is_some()
                || after.is_some()
                || limit.is_some() =>
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
        NativeCommand::QueryRepositoryPaths
            if instance.is_none() || prefix.is_none() || limit.is_none() =>
        {
            return Err(CliError::usage(
                "native query-repository-paths needs --instance, --prefix and --limit".into(),
            ));
        }
        NativeCommand::QueryRepositoryPaths
            if query_text.is_some()
                || syntax.is_some()
                || top_k.is_some()
                || output != NativeOutput::Json =>
        {
            return Err(CliError::usage(
                "native query-repository-paths accepts path flags and --output json only".into(),
            ));
        }
        NativeCommand::QueryRepositoryPaths
            if !limit.is_some_and(|limit| (1..=128).contains(&limit)) =>
        {
            return Err(CliError::usage("native --limit must be 1..=128".into()));
        }
        NativeCommand::QueryRepositoryPaths
            if after.as_ref().is_some_and(|cursor| {
                cursor.view.workspace != workspace || cursor.view.epoch != epoch
            }) =>
        {
            return Err(CliError::usage(
                "native --after-json view differs from --workspace or --epoch".into(),
            ));
        }
        NativeCommand::ReadView
        | NativeCommand::SearchText
        | NativeCommand::QueryRepositoryPaths => {}
    }
    Ok(NativeOptions {
        command,
        socket,
        workspace,
        epoch,
        instance,
        query_text,
        syntax: syntax.unwrap_or(TextQuerySyntax::Native),
        top_k,
        prefix,
        after,
        limit,
        output,
    })
}

fn map_error(error: &NativeUdsError, socket: &Path) -> CliError {
    let detail = format!("native socket {}: {error}", socket.display());
    match error {
        NativeUdsError::Transport(_) | NativeUdsError::PeerBindingRefused(_) => {
            CliError::transport(detail)
        }
        NativeUdsError::Protocol(_) => CliError::protocol(detail),
        NativeUdsError::Refused(_) => CliError::remote(detail),
        NativeUdsError::Unknown(_) => CliError {
            exit_code: EXIT_TRANSPORT,
            message: detail,
            unsupported: None,
        },
    }
}

pub(super) fn run(args: &[String], stdout: &mut dyn Write) -> CliResult<ExitCode> {
    if args.first().is_some_and(|command| command == "history") {
        return history::run(args, stdout);
    }
    if args
        .first()
        .is_some_and(|command| read_session::handles(command))
    {
        return read_session::run(args, stdout);
    }
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
                syntax: parsed.syntax,
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
        NativeCommand::QueryRepositoryPaths => {
            let instance = parsed
                .instance
                .ok_or_else(|| CliError::usage("native instance missing".into()))?;
            let prefix = parsed
                .prefix
                .ok_or_else(|| CliError::usage("native prefix missing".into()))?;
            let limit = parsed
                .limit
                .ok_or_else(|| CliError::usage("native limit missing".into()))?;
            let selection = parsed
                .after
                .as_ref()
                .map_or(selection, |cursor| ViewSelection::Exact(cursor.view));
            let page = client
                .query_repository_paths(selection, instance, prefix, parsed.after, limit, &budget)
                .map_err(|error| map_error(&error, &parsed.socket))?;
            let rendered = serde_json::to_string_pretty(&page).map_err(|error| {
                CliError::protocol(format!("native repository path encoding: {error}"))
            })?;
            write_stdout(stdout, &rendered)?;
        }
    }
    Ok(ExitCode::SUCCESS)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn native_text_cli_preserves_the_selected_syntax() -> CliResult<()> {
        let mut args = [
            "search-text",
            "--socket",
            "/tmp/native-text.sock",
            "--workspace",
            &"01".repeat(64),
            "--epoch",
            "1",
            "--instance",
            &"02".repeat(16),
            "--query-text",
            "path:^src/ needle",
            "--top-k",
            "5",
        ]
        .map(str::to_string)
        .to_vec();
        assert_eq!(parse(&args)?.syntax, TextQuerySyntax::Native);
        args.extend(["--syntax".into(), "sourcegraph".into()]);
        assert_eq!(parse(&args)?.syntax, TextQuerySyntax::Sourcegraph);
        args.extend(["--syntax".into(), "native".into()]);
        assert!(parse(&args).is_err());
        Ok(())
    }

    #[test]
    fn repository_path_cli_binds_json_cursor_to_workspace_and_epoch() -> CliResult<()> {
        use quanta_index_contract::index::{Digest, ViewRef};

        let cursor = NativeRepositoryPathCursor {
            view: ViewRef {
                workspace: workspace(&"01".repeat(64))?,
                epoch: 1,
                sequence: 4,
                digest: Digest([3; 32]),
            },
            instance: IndexInstanceId([2; 16]),
            prefix: "src/".into(),
            path: "src/a.rs".into(),
            repository: Some(
                quanta_index_contract::RepoId::new("corp-a")
                    .map_err(|error| CliError::usage(format!("test repository: {error}")))?,
            ),
        };
        let mut args = [
            "query-repository-paths",
            "--socket",
            "/tmp/native-paths.sock",
            "--workspace",
            &"01".repeat(64),
            "--epoch",
            "1",
            "--instance",
            &"02".repeat(16),
            "--prefix",
            "src/",
            "--limit",
            "2",
        ]
        .map(str::to_string)
        .to_vec();
        args.extend([
            "--after-json".into(),
            serde_json::to_string(&cursor)
                .map_err(|error| CliError::usage(format!("test cursor: {error}")))?,
        ]);
        let parsed = parse(&args)?;
        assert!(matches!(
            parsed.command,
            NativeCommand::QueryRepositoryPaths
        ));
        assert_eq!(parsed.output, NativeOutput::Json);
        assert_eq!(parsed.after, Some(cursor));
        args[6] = "2".into();
        assert!(parse(&args).is_err());
        Ok(())
    }

    #[test]
    fn native_peer_refusal_keeps_transport_exit_and_process_cause() {
        let error = map_error(
            &NativeUdsError::PeerBindingRefused(
                quanta_index_ipc::PeerBindingRefusalV1::ForeignProcess,
            ),
            Path::new("/tmp/native-peer.sock"),
        );
        assert_eq!(error.exit_code, EXIT_TRANSPORT);
        assert!(error.unsupported.is_none());
        assert!(error.message.contains("/tmp/native-peer.sock"));
        assert!(
            error
                .message
                .contains("connected peer differs from the admitted process")
        );
    }
}
