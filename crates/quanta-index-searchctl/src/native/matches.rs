//! Native text matches: every match of one literal or regex, one page at a
//! time, through the public SDK on a Native connection.
//!
//! A page that returns a cursor prints the `--after` token that continues it
//! on the same view. The continuation reads that view only while the daemon
//! still retains it; this process releases its own read session on exit.

use std::{collections::VecDeque, fmt::Write as _, io::Write, path::PathBuf, process::ExitCode};

use quanta_index_contract::{
    ContinuationTokenV2, QueryConstraintSetV1,
    index::{IndexInstanceId, ViewSelection, WorkspaceKey},
    native_protocol::{NativeTextMatchCase, NativeTextMatchPattern, NativeTextMatchQuery},
};
use quanta_index_sdk::{ConnectOptions, NativeTextMatchesResponse, QuantaIndex};

use super::{NativeOutput, bytes, set_once, value, workspace};
use crate::{CliError, CliResult, map_sdk_error, write_stdout};

pub(super) const COMMAND: &str = "matches";

pub(super) fn handles(command: &str) -> bool {
    command == COMMAND
}

#[derive(Debug, PartialEq)]
struct MatchesOptions {
    socket: PathBuf,
    workspace: WorkspaceKey,
    epoch: u64,
    query: NativeTextMatchQuery,
    limit: u32,
    instance: Option<IndexInstanceId>,
    after: Option<ContinuationTokenV2>,
    output: NativeOutput,
}

fn usage(message: impl Into<String>) -> CliError {
    CliError::usage(message.into())
}

fn parse(args: &[String]) -> CliResult<MatchesOptions> {
    let mut rest: VecDeque<String> = args.iter().cloned().collect();
    if rest.pop_front().as_deref() != Some(COMMAND) {
        return Err(usage("native matches command missing"));
    }
    let mut socket = None;
    let mut workspace_key = None;
    let mut epoch = None;
    let mut pattern = None;
    let mut ignore_case = false;
    let mut limit = None;
    let mut instance = None;
    let mut after = None;
    let mut output = None;
    while let Some(flag) = rest.pop_front() {
        match flag.as_str() {
            "--socket" => set_once(&mut socket, PathBuf::from(value(&mut rest, &flag)?), &flag)?,
            "--workspace" => set_once(
                &mut workspace_key,
                workspace(&value(&mut rest, &flag)?)?,
                &flag,
            )?,
            "--epoch" => {
                let raw = value(&mut rest, &flag)?;
                let parsed = raw
                    .parse::<u64>()
                    .map_err(|error| usage(format!("invalid --epoch: {error}")))?;
                set_once(&mut epoch, parsed, &flag)?;
            }
            "--literal" => set_once(
                &mut pattern,
                NativeTextMatchPattern::Literal(value(&mut rest, &flag)?),
                "--literal/--regex",
            )?,
            "--regex" => set_once(
                &mut pattern,
                NativeTextMatchPattern::Regex(value(&mut rest, &flag)?),
                "--literal/--regex",
            )?,
            "--ignore-case" => {
                if ignore_case {
                    return Err(usage("--ignore-case may be given only once"));
                }
                ignore_case = true;
            }
            "--limit" => {
                let raw = value(&mut rest, &flag)?;
                let parsed = raw
                    .parse::<u32>()
                    .map_err(|error| usage(format!("invalid --limit: {error}")))?;
                set_once(&mut limit, parsed, &flag)?;
            }
            "--instance" => set_once(
                &mut instance,
                IndexInstanceId(bytes::<16>(&value(&mut rest, &flag)?, "instance")?),
                &flag,
            )?,
            "--after" => {
                let raw = value(&mut rest, &flag)?;
                let token = ContinuationTokenV2::new(raw)
                    .map_err(|error| usage(format!("invalid --after: {error}")))?;
                set_once(&mut after, token, &flag)?;
            }
            "--output" => {
                let parsed = match value(&mut rest, &flag)?.as_str() {
                    "pretty" => NativeOutput::Pretty,
                    "json" => NativeOutput::Json,
                    other => return Err(usage(format!("matches output {other} is unsupported"))),
                };
                set_once(&mut output, parsed, &flag)?;
            }
            other => return Err(usage(format!("unknown matches argument {other}"))),
        }
    }
    let socket = socket.ok_or_else(|| usage("matches requires --socket PATH"))?;
    if !socket.is_absolute() {
        return Err(usage("matches --socket must be absolute"));
    }
    let query = NativeTextMatchQuery {
        pattern: pattern.ok_or_else(|| usage("matches requires --literal TEXT or --regex TEXT"))?,
        case: if ignore_case {
            NativeTextMatchCase::Insensitive
        } else {
            NativeTextMatchCase::Sensitive
        },
        constraints: QueryConstraintSetV1::unconstrained(),
    };
    query.validate().map_err(usage)?;
    Ok(MatchesOptions {
        socket,
        workspace: workspace_key.ok_or_else(|| usage("matches requires --workspace HEX"))?,
        epoch: epoch.ok_or_else(|| usage("matches requires --epoch N"))?,
        query,
        limit: limit.ok_or_else(|| usage("matches requires --limit N"))?,
        instance,
        after,
        output: output.unwrap_or(NativeOutput::Pretty),
    })
}

pub(super) fn run(args: &[String], stdout: &mut dyn Write) -> CliResult<ExitCode> {
    let options = parse(args)?;
    let mut connect = ConnectOptions::from_native_socket(
        options.socket.clone(),
        ViewSelection::Latest {
            workspace: options.workspace,
            epoch: options.epoch,
        },
    );
    if let Some(instance) = options.instance {
        connect = connect.with_native_text_instance(instance);
    }
    let client = QuantaIndex::connect(connect).map_err(map_sdk_error)?;
    let response = client
        .lexical()
        .matches(options.query, options.limit, options.after.as_ref())
        .map_err(map_sdk_error)?;
    let rendered = match options.output {
        NativeOutput::Json => serde_json::to_string_pretty(&response)
            .map_err(|error| CliError::protocol(format!("matches encoding: {error}")))?,
        NativeOutput::Pretty => render_pretty(&response)?,
    };
    write_stdout(stdout, &rendered)?;
    Ok(ExitCode::SUCCESS)
}

/// One `repository<TAB>path<TAB>start..end` line per match; `-` names the
/// unscoped repository slot.
fn render_pretty(response: &NativeTextMatchesResponse) -> CliResult<String> {
    let render = |error: std::fmt::Error| CliError::protocol(format!("matches rendering: {error}"));
    let mut value = String::new();
    writeln!(value, "view: {}", response.view.sequence).map_err(render)?;
    for row in &response.matches {
        writeln!(
            value,
            "{}\t{}\t{}..{}",
            row.repository
                .as_ref()
                .map_or("-", |repository| repository.as_str()),
            row.path,
            row.range.start,
            row.range.end
        )
        .map_err(render)?;
    }
    if let Some(cursor) = &response.next_cursor {
        writeln!(value, "next: --after {}", cursor.as_str()).map_err(render)?;
    }
    Ok(value)
}

#[cfg(test)]
mod tests {
    use super::*;

    const WORKSPACE: &str = "01010101010101010101010101010101020202020202020202020202020202020303030303030303030303030303030304040404040404040404040404040404";

    fn args(extra: &[&str]) -> Vec<String> {
        [
            COMMAND,
            "--socket",
            "/tmp/native.sock",
            "--workspace",
            WORKSPACE,
            "--epoch",
            "1",
            "--limit",
            "2",
        ]
        .iter()
        .chain(extra)
        .map(|value| (*value).to_owned())
        .collect()
    }

    fn usage_message(result: CliResult<MatchesOptions>) -> String {
        match result {
            Err(error) => error.message,
            Ok(options) => panic!("expected a usage refusal, parsed {options:?}"),
        }
    }

    #[test]
    fn literal_regex_case_and_continuation_flags_parse() {
        let literal = parse(&args(&["--literal", "a.b"])).expect("literal parses");
        assert_eq!(
            literal.query.pattern,
            NativeTextMatchPattern::Literal("a.b".into())
        );
        assert_eq!(literal.query.case, NativeTextMatchCase::Sensitive);
        assert_eq!(literal.workspace.slot.0, [4; 16]);
        assert_eq!((literal.epoch, literal.limit), (1, 2));
        assert_eq!(literal.output, NativeOutput::Pretty);
        let regex = parse(&args(&[
            "--regex",
            "fn \\w+",
            "--ignore-case",
            "--after",
            "native-text-matches:00",
            "--output",
            "json",
        ]))
        .expect("regex parses");
        assert_eq!(
            regex.query.pattern,
            NativeTextMatchPattern::Regex("fn \\w+".into())
        );
        assert_eq!(regex.query.case, NativeTextMatchCase::Insensitive);
        assert_eq!(
            regex.after.as_ref().map(ContinuationTokenV2::as_str),
            Some("native-text-matches:00")
        );
        assert_eq!(regex.output, NativeOutput::Json);
    }

    #[test]
    fn missing_conflicting_or_empty_patterns_are_usage_errors() {
        for (flags, reason) in [
            (&[][..], "--literal TEXT or --regex TEXT"),
            (&["--literal", "a", "--regex", "b"], "only once"),
            (&["--literal", ""], "1..=4096"),
            (
                &["--literal", "a", "--ignore-case", "--ignore-case"],
                "only once",
            ),
            (
                &["--literal", "a", "--top-k", "1"],
                "unknown matches argument",
            ),
        ] {
            let message = usage_message(parse(&args(flags)));
            assert!(message.contains(reason), "{flags:?}: {message}");
        }
        let relative = usage_message(parse(&[
            COMMAND.to_owned(),
            "--socket".into(),
            "native.sock".into(),
        ]));
        assert!(relative.contains("absolute"), "{relative}");
    }
}
