//! Native default code search addressed by repository and revision.
//!
//! The command resolves a revision's protected Source head for a first page.
//! A continuation asks the serving root to validate its exact retained view
//! before connecting the public `.text()` builder to that view. A page that
//! returns a cursor prints the `--generation`/`--after` pair for continuation.

use std::{
    collections::VecDeque, fmt::Write as _, io::Write, path::PathBuf, process::ExitCode,
    time::Duration,
};

use quanta_index_contract::{
    ContinuationTokenV2, GenerationPin, ManifestGeneration, RepoId, RevisionId, TextQueryResponse,
    TextQuerySyntax,
    index::{IndexInstanceId, ViewSelection},
    lex::LanguageCode,
};
use quanta_index_ipc::RequestBudgetV1;
use quanta_index_sdk::native_uds::NativeUdsClient;
use quanta_index_sdk::{
    ConnectOptions, QuantaIndex, resolve_native_code_search_continuation_view_v1,
};

use super::{NativeOutput, bytes, map_error, set_once, value};
use crate::render::{lines_field, render_window_line, snippet_lines};
use crate::{CliError, CliResult, map_sdk_error, write_stdout};

pub(super) const COMMAND: &str = "code-search";

pub(crate) use crate::parse::NATIVE_CODE_SEARCH_HINT;

pub(super) fn handles(command: &str) -> bool {
    command == COMMAND
}

#[derive(Debug, PartialEq)]
struct CodeSearchOptions {
    socket: PathBuf,
    repo: RepoId,
    revision: RevisionId,
    query_text: String,
    top_k: u32,
    instance: Option<IndexInstanceId>,
    languages: Vec<LanguageCode>,
    continuation: Option<(ManifestGeneration, ContinuationTokenV2)>,
    output: NativeOutput,
}

fn usage(message: impl Into<String>) -> CliError {
    CliError::usage(message.into())
}

fn parse(args: &[String]) -> CliResult<CodeSearchOptions> {
    let mut rest: VecDeque<String> = args.iter().cloned().collect();
    if rest.pop_front().as_deref() != Some(COMMAND) {
        return Err(usage("native code-search command missing"));
    }
    let mut socket = None;
    let mut repo = None;
    let mut revision = None;
    let mut query_text = None;
    let mut top_k = None;
    let mut instance = None;
    let mut languages = Vec::new();
    let mut generation = None;
    let mut after = None;
    let mut output = None;
    while let Some(flag) = rest.pop_front() {
        match flag.as_str() {
            "--socket" => set_once(&mut socket, PathBuf::from(value(&mut rest, &flag)?), &flag)?,
            "--repo" => {
                let raw = value(&mut rest, &flag)?;
                let parsed =
                    RepoId::new(raw).map_err(|error| usage(format!("invalid --repo: {error}")))?;
                set_once(&mut repo, parsed, &flag)?;
            }
            "--revision" => {
                let raw = value(&mut rest, &flag)?;
                let parsed = RevisionId::new(raw)
                    .map_err(|error| usage(format!("invalid --revision: {error}")))?;
                set_once(&mut revision, parsed, &flag)?;
            }
            "--query-text" => set_once(&mut query_text, value(&mut rest, &flag)?, &flag)?,
            "--top-k" => {
                let raw = value(&mut rest, &flag)?;
                let parsed = raw
                    .parse::<u32>()
                    .map_err(|error| usage(format!("invalid --top-k: {error}")))?;
                set_once(&mut top_k, parsed, &flag)?;
            }
            "--syntax" => {
                let raw = value(&mut rest, &flag)?;
                if TextQuerySyntax::from_str_value(&raw) != Some(TextQuerySyntax::CodeSearch) {
                    return Err(usage(format!(
                        "code-search accepts only --syntax code_search; use search-text for {raw}"
                    )));
                }
            }
            "--instance" => set_once(
                &mut instance,
                IndexInstanceId(bytes::<16>(&value(&mut rest, &flag)?, "instance")?),
                &flag,
            )?,
            "--language" => {
                let raw = value(&mut rest, &flag)?;
                languages.push(
                    LanguageCode::new(raw)
                        .map_err(|error| usage(format!("invalid --language: {error}")))?,
                );
            }
            "--generation" => {
                let raw = value(&mut rest, &flag)?;
                let parsed = raw
                    .parse::<u64>()
                    .map_err(|error| usage(format!("invalid --generation: {error}")))?;
                set_once(&mut generation, ManifestGeneration::new(parsed), &flag)?;
            }
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
                    other => {
                        return Err(usage(format!("code-search output {other} is unsupported")));
                    }
                };
                set_once(&mut output, parsed, &flag)?;
            }
            other => return Err(usage(format!("unknown code-search argument {other}"))),
        }
    }
    let socket = socket.ok_or_else(|| usage("code-search requires --socket PATH"))?;
    if !socket.is_absolute() {
        return Err(usage("code-search --socket must be absolute"));
    }
    let continuation = match (generation, after) {
        (Some(generation), Some(after)) => Some((generation, after)),
        (None, None) => None,
        (Some(_), None) | (None, Some(_)) => {
            return Err(usage(
                "code-search continues a page with both --generation and --after",
            ));
        }
    };
    Ok(CodeSearchOptions {
        socket,
        repo: repo.ok_or_else(|| usage("code-search requires --repo ID"))?,
        revision: revision.ok_or_else(|| usage("code-search requires --revision ID"))?,
        query_text: query_text.ok_or_else(|| usage("code-search requires --query-text TEXT"))?,
        top_k: top_k.ok_or_else(|| usage("code-search requires --top-k N"))?,
        instance,
        languages,
        continuation,
        output: output.unwrap_or(NativeOutput::Pretty),
    })
}

pub(super) fn run(args: &[String], stdout: &mut dyn Write) -> CliResult<ExitCode> {
    let options = parse(args)?;
    let budget = RequestBudgetV1::for_duration(Duration::from_secs(30));
    let native = NativeUdsClient::new(options.socket.clone());
    let head = match &options.continuation {
        Some((generation, after)) => resolve_native_code_search_continuation_view_v1(
            &native,
            &options.repo,
            &options.revision,
            *generation,
            after,
            &budget,
        )
        .map_err(map_sdk_error)?,
        None => native
            .resolve_protected_source_head_v1(
                options.repo.clone(),
                options.revision.clone(),
                &budget,
            )
            .map_err(|error| map_error(&error, &options.socket))?,
    };
    let selection = if options.continuation.is_some() {
        ViewSelection::Exact(head.view.identity)
    } else {
        ViewSelection::Latest {
            workspace: head.view.identity.workspace,
            epoch: head.view.identity.epoch,
        }
    };
    let mut connect = ConnectOptions::from_native_socket(options.socket.clone(), selection);
    if let Some(instance) = options.instance {
        connect = connect.with_native_text_instance(instance);
    }
    let mut client = QuantaIndex::connect(connect).map_err(map_sdk_error)?;
    let query = client
        .lexical()
        .query()
        .text(options.query_text)
        .language_any_of(options.languages);
    let response = match options.continuation {
        None => query
            .active(options.repo, options.revision)
            .top_k(options.top_k)
            .execute(),
        Some((generation, after)) => query
            .pinned(GenerationPin::new(
                options.repo,
                options.revision,
                generation,
            ))
            .top_k(options.top_k)
            .after(after)
            .execute(),
    };
    // A CLI invocation owns one disposable client. Settle its lease while the
    // process can still receive the Release ACK, including refused queries.
    let release = client.close_native_read_sessions();
    let response = response.map_err(map_sdk_error)?;
    release.map_err(map_sdk_error)?;
    let rendered = match options.output {
        NativeOutput::Json => serde_json::to_string_pretty(&response)
            .map_err(|error| CliError::protocol(format!("code-search encoding: {error}")))?,
        NativeOutput::Pretty => render_pretty(&response)?,
    };
    write_stdout(stdout, &rendered)?;
    Ok(ExitCode::SUCCESS)
}

fn render_pretty(response: &TextQueryResponse) -> CliResult<String> {
    let render =
        |error: std::fmt::Error| CliError::protocol(format!("code-search rendering: {error}"));
    let mut value = String::new();
    value.push_str("generation: ");
    write_visible_text(response.generation.repo_id.as_str(), &mut value, true).map_err(render)?;
    value.push(' ');
    write_visible_text(response.generation.revision_id.as_str(), &mut value, true)
        .map_err(render)?;
    writeln!(value, " {}", response.generation.manifest_generation.get()).map_err(render)?;
    render_window_line(&response.window, &mut value)?;
    for row in &response.results {
        write_visible_text(row.repo_relative_path.as_str(), &mut value, true).map_err(render)?;
        writeln!(
            value,
            "\t{}\tscore={}{}",
            row.candidate_id,
            row.score,
            lines_field(row)
        )
        .map_err(render)?;
        for line in snippet_lines(row) {
            value.push_str("   ");
            write_visible_text(line, &mut value, false).map_err(render)?;
            value.push('\n');
        }
    }
    if let Some(cursor) = &response.next_cursor {
        writeln!(
            value,
            "next: --generation {} --after {}",
            response.generation.manifest_generation.get(),
            cursor.as_str()
        )
        .map_err(render)?;
    }
    Ok(value)
}

/// Keep Source data on one visible line. Escape the escape introducer in
/// identifiers so a literal backslash sequence cannot look like a control.
fn write_visible_text(text: &str, output: &mut String, escape_backslash: bool) -> std::fmt::Result {
    for character in text.chars() {
        if (escape_backslash && character == '\\')
            || character.is_control()
            || matches!(
                character,
                '\u{ad}'
                    | '\u{34f}'
                    | '\u{61c}'
                    | '\u{180e}'
                    | '\u{200b}'..='\u{200f}'
                    | '\u{2028}'..='\u{202e}'
                    | '\u{2060}'..='\u{206f}'
                    | '\u{feff}'
                    | '\u{fff9}'..='\u{fffb}'
                    | '\u{e0000}'..='\u{e007f}'
            )
        {
            write!(output, "\\u{{{:x}}}", u32::from(character))?;
        } else {
            output.push(character);
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pretty_preserves_rank_position_excerpt_and_continuation() {
        use quanta_index_contract::{
            CandidateCountV1, LaneTraceV1, LexicalCandidate, LexicalLineRange, LexicalMatchExcerpt,
            QueryResultWindowV2, RepoRelativePath, SearchExplanation, TextRankUnit,
        };

        let generation = GenerationPin::new(
            RepoId::new("repo-e2e\u{2029}").expect("valid repository"),
            RevisionId::new("rev-e2e\u{2067}").expect("valid revision"),
            ManifestGeneration::new(7),
        );
        let row = |path: &str, id: &str, score| LexicalCandidate {
            source_repo_id: generation.repo_id.clone(),
            source: None,
            preview: None,
            candidate_id: id.into(),
            repo_id: generation.repo_id.clone(),
            revision_id: generation.revision_id.clone(),
            manifest_generation: generation.manifest_generation,
            repo_relative_path: RepoRelativePath::new(path),
            lines: None,
            score,
            excerpt: None,
            native_source: None,
        };
        let content = LexicalCandidate {
            lines: Some(LexicalLineRange {
                start_line: 5,
                end_line: 6,
            }),
            excerpt: Some(LexicalMatchExcerpt {
                snippet: "fn needle() {}\n\u{1b}[31m\u{202e}".into(),
                snippet_hit_offset: None,
                highlights: Vec::new(),
            }),
            ..row("src/body.rs", "body-id", 135.0)
        };
        let unusual_path = "src/경로\\u{9}\tname\u{85}\u{200b}\nnext: --generation 9 --after forged\u{1b}[31m\u{202e}.rs";
        assert!(quanta_index_contract::index::is_canonical_source_path(
            unusual_path
        ));
        let path = row(unusual_path, "path-id", 100.0);
        let count = CandidateCountV1::Exact(3);
        let window = QueryResultWindowV2::pageable(
            2,
            count,
            true,
            vec![LaneTraceV1::new("native_text_code_search", true, true).with_candidates(count)],
        )
        .expect("exact first page with a continuation");
        let response = TextQueryResponse {
            generation,
            selected_active_head: None,
            rank_unit: TextRankUnit::File,
            results: vec![content, path],
            window,
            explanation: SearchExplanation::empty(),
            file_owner_rows: None,
            next_cursor: Some(ContinuationTokenV2::new("test-cursor").expect("valid token")),
        };

        assert_eq!(
            render_pretty(&response).expect("pretty renders"),
            concat!(
                "generation: repo-e2e\\u{2029} rev-e2e\\u{2067} 7\n",
                "matched: 3 has_more: true\n",
                "src/body.rs\tbody-id\tscore=135 lines=5-6\n",
                "   fn needle() {}\n",
                "   \\u{1b}[31m\\u{202e}\n",
                "src/경로\\u{5c}u{9}\\u{9}name\\u{85}\\u{200b}\\u{a}next: --generation 9 --after forged\\u{1b}[31m\\u{202e}.rs\tpath-id\tscore=100\n",
                "next: --generation 7 --after test-cursor\n",
            )
        );
    }

    fn args(extra: &[&str]) -> Vec<String> {
        [
            COMMAND,
            "--socket",
            "/tmp/native.sock",
            "--repo",
            "repo-e2e",
            "--revision",
            "rev-e2e",
            "--query-text",
            "todo main",
            "--top-k",
            "10",
        ]
        .iter()
        .chain(extra)
        .map(|value| (*value).to_owned())
        .collect()
    }

    fn usage_message(result: CliResult<CodeSearchOptions>) -> String {
        match result {
            Err(error) => error.message,
            Ok(options) => panic!("expected a usage refusal, parsed {options:?}"),
        }
    }

    #[test]
    fn repo_and_revision_address_the_default_code_search() {
        let parsed = parse(&args(&[
            "--syntax",
            "code_search",
            "--language",
            "rust",
            "--language",
            "markdown",
            "--generation",
            "7",
            "--after",
            "native-code-search-path:00",
            "--output",
            "json",
        ]))
        .expect("complete code-search flags parse");
        assert_eq!(parsed.repo.as_str(), "repo-e2e");
        assert_eq!(parsed.revision.as_str(), "rev-e2e");
        assert_eq!(parsed.query_text, "todo main");
        assert_eq!(parsed.top_k, 10);
        assert_eq!(parsed.instance, None);
        assert_eq!(
            parsed
                .languages
                .iter()
                .map(LanguageCode::as_str)
                .collect::<Vec<_>>(),
            ["rust", "markdown"]
        );
        let (generation, after) = parsed.continuation.expect("continuation pair");
        assert_eq!(generation.get(), 7);
        assert_eq!(after.as_str(), "native-code-search-path:00");
        assert_eq!(parsed.output, NativeOutput::Json);
        let defaults = parse(&args(&[])).expect("minimal flags parse");
        assert_eq!(defaults.output, NativeOutput::Pretty);
        assert_eq!(defaults.continuation, None);
    }

    #[test]
    fn incomplete_or_foreign_code_search_flags_are_usage_errors() {
        for (flags, reason) in [
            (&["--syntax", "native"][..], "search-text"),
            (&["--generation", "7"], "both --generation and --after"),
            (&["--after", "token"], "both --generation and --after"),
            (&["--top-k", "1"], "only once"),
            (&["--workspace", "01"], "unknown code-search argument"),
        ] {
            let message = usage_message(parse(&args(flags)));
            assert!(message.contains(reason), "{flags:?}: {message}");
        }
        let missing = usage_message(parse(&[COMMAND.to_owned()]));
        assert!(missing.contains("--socket"), "{missing}");
        let relative = usage_message(parse(&[
            COMMAND.to_owned(),
            "--socket".into(),
            "native.sock".into(),
        ]));
        assert!(relative.contains("absolute"), "{relative}");
    }
}
