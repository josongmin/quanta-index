#![forbid(unsafe_code)]
#![deny(unused_must_use)]
#![deny(clippy::let_underscore_must_use)]
#![deny(clippy::map_err_ignore)]

use std::io::Write;
use std::process::ExitCode;

use quanta_index_contract::{
    ExplainCandidateV1, RepoId, RevisionId, SearchPlaneQueryIpcResponse,
    SearchPlaneQueryIpcResponseEnvelope, ipc::SearchPlaneTrackKind,
};
use quanta_index_sdk::{QuantaIndex, SdkError};

mod parse;
mod render;

#[cfg(test)]
use parse::parse_focus_subject;
use parse::{CliRequest, CommandKind, OutputMode, ParsedCommand};

use render::{
    render_doctor, render_generation_status, render_metrics, render_process_readiness,
    render_quarantine_discard, render_quarantine_inventory, render_remote_error_text,
    render_request_events, render_response, validate_response_kind,
};

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
    // Process readiness, generation status and doctor dispatch via CONTROL, not the
    // query plane. Branch them here so the query path (`dispatch_query_request`
    // -> `validate_response_kind` -> `render_response`) stays untouched.
    match request {
        CliRequest::GenerationStatus {
            repo_id,
            revision_id,
        } => {
            let report = client
                .generations()
                .status(repo_id, revision_id)
                .map_err(map_sdk_error)?;
            write_stdout(stdout, &render_generation_status(&report, output)?)?;
            Ok(ExitCode::SUCCESS)
        }
        CliRequest::ProcessReadiness => {
            let report = client
                .observability()
                .process_readiness()
                .map_err(map_sdk_error)?;
            write_stdout(stdout, &render_process_readiness(&report, output)?)?;
            Ok(ExitCode::SUCCESS)
        }
        CliRequest::RequestEvents { plane, limit } => {
            let events = client
                .observability()
                .request_events(plane, limit)
                .map_err(map_sdk_error)?;
            write_stdout(stdout, &render_request_events(&events, output)?)?;
            Ok(ExitCode::SUCCESS)
        }
        CliRequest::Doctor {
            repo_id,
            revision_id,
        } => {
            let report = build_doctor_report(&client, repo_id, revision_id)?;
            write_stdout(stdout, &render_doctor(&report, output)?)?;
            Ok(ExitCode::SUCCESS)
        }
        CliRequest::Metrics => {
            let snapshot = client
                .observability()
                .metrics_snapshot()
                .map_err(map_sdk_error)?;
            write_stdout(stdout, &render_metrics(&snapshot, output)?)?;
            Ok(ExitCode::SUCCESS)
        }
        CliRequest::QuarantineList => {
            let inventory = client.quarantine().inventory().map_err(map_sdk_error)?;
            write_stdout(stdout, &render_quarantine_inventory(&inventory, output)?)?;
            Ok(ExitCode::SUCCESS)
        }
        CliRequest::QuarantineDiscard(target) => {
            let ack = client
                .quarantine()
                .discard(&target)
                .map_err(map_sdk_error)?;
            write_stdout(stdout, &render_quarantine_discard(&ack, output)?)?;
            Ok(ExitCode::SUCCESS)
        }
        query_request @ (CliRequest::Lexical(_)
        | CliRequest::Symbol(_)
        | CliRequest::Semantic(_)
        | CliRequest::Hybrid(_)
        | CliRequest::HybridSeed(_)
        | CliRequest::Explain { .. }
        | CliRequest::RepoMap(_)
        | CliRequest::RuntimeMetadata(_)
        | CliRequest::History(_)
        | CliRequest::Structural(_)) => {
            let response = dispatch_query_request(&client, query_request)?;
            validate_response_kind(kind, &response)?;
            render_response(output, &response, stdout)?;
            Ok(ExitCode::SUCCESS)
        }
    }
}

/// Write a fully-rendered CLI string to stdout.
///
/// Maps any I/O failure to a typed transport error. Shared by the control-plane
/// renderers (`readiness` / `doctor`) so the failure shape cannot drift.
fn write_stdout(stdout: &mut dyn Write, rendered: &str) -> CliResult<()> {
    stdout
        .write_all(rendered.as_bytes())
        .map_err(|err| CliError::transport(format!("failed writing stdout: {err}")))
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
        CliRequest::Hybrid(hybrid) => SearchPlaneQueryIpcResponse::Hybrid(
            client
                .search()
                .hybrid_request(hybrid)
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
            text_query,
            semantic_query_text,
        } => SearchPlaneQueryIpcResponse::Explain(
            match (*candidate, text_query, semantic_query_text) {
                (ExplainCandidateV1::Lexical(candidate), Some(text_query), None) => client
                    .search()
                    .explain_under_query(generation, candidate, text_query),
                (ExplainCandidateV1::Lexical(candidate), None, None) => {
                    client.search().explain(generation, candidate)
                }
                (ExplainCandidateV1::Hybrid(row), Some(text_query), Some(semantic_query_text)) => {
                    client.search().explain_hybrid_under_queries(
                        generation,
                        row,
                        text_query,
                        semantic_query_text,
                    )
                }
                (ExplainCandidateV1::Hybrid(_), _, _) => {
                    return Err(CliError::usage(
                        "a hybrid candidate explains under both its queries; pass --syntax, --query-text and --semantic-query-text"
                            .to_string(),
                    ));
                }
                (ExplainCandidateV1::Lexical(_), _, Some(_)) => {
                    return Err(CliError::usage(
                        "a lexical candidate has no dense lane; drop --semantic-query-text"
                            .to_string(),
                    ));
                }
            }
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
        // `readiness` / `doctor` are control-plane commands; `run_inner` branches
        // them before reaching the query dispatcher. Reaching here is a routing
        // bug, so fail-closed with a typed protocol error rather than fabricate a
        // query.
        CliRequest::GenerationStatus { .. }
        | CliRequest::ProcessReadiness
        | CliRequest::RequestEvents { .. }
        | CliRequest::Doctor { .. }
        | CliRequest::Metrics
        | CliRequest::QuarantineList
        | CliRequest::QuarantineDiscard(_) => {
            return Err(CliError::protocol(
                "readiness/events/generation-status/doctor/metrics/quarantine are control-plane commands and must not reach the query dispatcher"
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
        SdkError::Binding {
            route,
            axis,
            expected,
            actual,
        } => CliError::protocol(format!(
            "response binding failed on route `{route}` axis `{axis}`: expected {expected}, got {actual}"
        )),
        SdkError::PlaneUnavailable { plane } => CliError::usage(format!(
            "{plane} transport is not configured for this client profile"
        )),
    }
}

/// J7Q-05: one activated track's slot in the composite [`DoctorReport`].
///
/// `manifest_generation` / `manifest_digest` come from the activation-catalog
/// listing (`generations().status`). `resolver_ok` records whether the
/// serve-time per-track resolver (`generations().current`) corroborated that
/// listing for this track — the cross-check `readiness` does not perform.
#[derive(Clone, Debug)]
struct DoctorTrack {
    track: SearchPlaneTrackKind,
    manifest_generation: u64,
    manifest_digest: String,
    resolver_ok: bool,
    resolver_note: String,
}

/// J7Q-05: composite read-only diagnosis for one `(repo, revision)` pair.
///
/// Fuses the activation-catalog listing with per-track serve-time resolution
/// into one operator answer. `serve_ready` is true only when at least one track
/// is activated AND every activated track resolved consistently. `all_resolvable`
/// isolates the resolver-corroboration verdict from the "is anything activated
/// at all" question, so an empty repo reads `serve_ready=false`,
/// `all_resolvable=true` (benign) — distinct from `serve_ready=false`,
/// `all_resolvable=false` (a real listing-vs-resolver inconsistency).
#[derive(Clone, Debug)]
struct DoctorReport {
    repo_id: String,
    revision_id: String,
    tracks: Vec<DoctorTrack>,
    serve_ready: bool,
    all_resolvable: bool,
}

/// J7Q-05: build the composite [`DoctorReport`] from two distinct control reads.
///
/// The catalog listing (`status`) and per-track serve-time resolution (`current`)
/// read the same activation catalog through different code, so corroborating them
/// catches a listing-vs-resolver divergence that `readiness` (listing only) cannot
/// see.
fn build_doctor_report(
    client: &QuantaIndex,
    repo_id: RepoId,
    revision_id: RevisionId,
) -> CliResult<DoctorReport> {
    // Authoritative listing: which tracks the activation catalog serves for this
    // pair, each with its (generation, manifest_digest). Any transport / protocol
    // / remote failure here propagates fail-closed (the daemon is unreachable or
    // the catalog read failed — never a fabricated "healthy" verdict).
    let status = client
        .generations()
        .status(repo_id, revision_id)
        .map_err(map_sdk_error)?;
    let mut tracks = Vec::with_capacity(status.tracks.len());
    let mut all_resolvable = true;
    for record in &status.tracks {
        let resolution = client.generations().current(
            status.repo_id.clone(),
            status.revision_id.clone(),
            record.track,
        );
        // Classify the resolver's answer. A divergent snapshot or a typed
        // NOT_READY for a *listed* track is a recorded diagnosis finding
        // (`resolver_ok=false`), not a default — the failure is surfaced, never
        // masked. Every other error propagates fail-closed.
        let (resolver_ok, resolver_note) = match resolution {
            Ok(snapshot)
                if snapshot.manifest_generation == record.manifest_generation
                    && snapshot.manifest_digest == record.manifest_digest =>
            {
                (
                    true,
                    "resolved: catalog listing and serve-time resolver agree".to_string(),
                )
            }
            Ok(snapshot) => (
                false,
                format!(
                    "divergence: catalog lists generation={} digest={} but serve-time resolver returned generation={} digest={}",
                    record.manifest_generation.get(),
                    record.manifest_digest,
                    snapshot.manifest_generation.get(),
                    snapshot.manifest_digest
                ),
            ),
            Err(error) if is_not_ready(&error) => (
                false,
                "serve-time resolver returned NOT_READY for a track the catalog lists as active"
                    .to_string(),
            ),
            Err(error) => return Err(map_sdk_error(error)),
        };
        if !resolver_ok {
            all_resolvable = false;
        }
        tracks.push(DoctorTrack {
            track: record.track,
            manifest_generation: record.manifest_generation.get(),
            manifest_digest: record.manifest_digest.clone(),
            resolver_ok,
            resolver_note,
        });
    }
    let serve_ready = !tracks.is_empty() && all_resolvable;
    Ok(DoctorReport {
        repo_id: status.repo_id.as_str().to_string(),
        revision_id: status.revision_id.as_str().to_string(),
        tracks,
        serve_ready,
        all_resolvable,
    })
}

/// Whether a resolver error is the typed `NOT_READY` diagnosis state.
///
/// A `NOT_READY` for a *listed* track is a finding (the catalog listed it but the
/// resolver will not serve it), not a transport failure to abort on. Match the
/// typed code exactly; every other `SdkError` (including other remote codes) stays
/// an abort in [`build_doctor_report`].
fn is_not_ready(error: &SdkError) -> bool {
    matches!(
        error,
        SdkError::Remote {
            code: quanta_index_contract::SearchPlaneErrorCodeV2::NotReady,
            ..
        }
    )
}

fn usage() -> &'static str {
    "\
quanta-index-searchctl

Global flags:
  --socket PATH
  --state-root PATH
  --output pretty|json|prometheus   (prometheus: `metrics` only)

Read-only subcommands:
  lexical          --repo-id ID --revision-id REV --manifest-generation N [--syntax code_search|native|sourcegraph] --query-text TEXT --top-k N [--cursor-json PATH|-]
  symbol           --repo-id ID --revision-id REV --manifest-generation N --syntax native|sourcegraph --query-text TEXT --top-k N [--cursor-json PATH|-]
  semantic         --repo-id ID --revision-id REV --manifest-generation N --query-text TEXT --top-k N [--scope-query TEXT --scope-syntax native|sourcegraph --scope-top-k N]
  hybrid           --repo-id ID --revision-id REV --manifest-generation N --syntax native|sourcegraph --query-text TEXT --semantic-query-text TEXT --top-k N
  hybrid-seed      --repo-id ID --revision-id REV --manifest-generation N --lexical-query TEXT --lexical-syntax native|sourcegraph --semantic-query TEXT --top-k N
  explain          --repo-id ID --revision-id REV --manifest-generation N --candidate-json PATH|- [--syntax native|sourcegraph --query-text TEXT]
  explain          --repo-id ID --revision-id REV --manifest-generation N --hybrid-candidate-json PATH|- --syntax native|sourcegraph --query-text TEXT --semantic-query-text TEXT --top-k N
  repomap          --repo-id ID --revision-id REV --manifest-generation N --query-text TEXT --top-k N --token-budget N [--focus-subject subject_identity:subject_doc_type]
    history          --repo-id ID --revision-id REV --manifest-generation N --syntax native|sourcegraph --query-text TEXT --top-k N --order recency|relevance, history          --repo-id ID --revision-id REV --manifest-generation N --syntax native|sourcegraph --query-text TEXT --top-k N [--cursor-json PATH|-], runtime-metadata --repo-id ID --revision-id REV --manifest-generation N --syntax native|sourcegraph --query-text TEXT --top-k N, runtime-metadata --repo-id ID --revision-id REV --manifest-generation N --syntax native|sourcegraph --query-text TEXT --top-k N [--cursor-json PATH|-], structural       --repo-id ID --revision-id REV --manifest-generation N --syntax native|sourcegraph --query-text TEXT --top-k N, structural       --repo-id ID --revision-id REV --manifest-generation N --syntax native|sourcegraph --query-text TEXT --top-k N [--cursor-json PATH|-],
  readiness
  events           --plane query|control|ingest --limit 1..1024
  generation-status --repo-id ID --revision-id REV
  doctor           --repo-id ID --revision-id REV
  metrics
  quarantine       list
  quarantine       discard --track lexical|semantic --path PATH --reason CODE [--detail TEXT]
  quarantine       discard --repomap-file NAME --reason TEXT

`hybrid` runs two independent, bounded lanes over the pinned generation — the
lexical lane over `--query-text` and the dense lane over the embedded
`--semantic-query-text` — and fuses their union by reciprocal rank fusion, so a
document with no lexical overlap can enter the page on dense relevance alone.
Each row carries its fused score and the rank and raw score every lane gave it.
`semantic --scope-query` is the other shape: the dense lane is confined to the
lexical universe the scope query matches (a lexical-scoped rerank), so a
document the scope does not match can never enter its page.

`explain --hybrid-candidate-json` re-derives a hybrid row against the index
under both queries it was fused for: the lexical lane through the engine's own
score trace, the dense lane by scoring the row's stored vector against the
embedded `--semantic-query-text`, and the fusion by re-running both lanes at
`--top-k` (the hybrid's fused top_k). The trace reports
`explain.score_reconciled`, `explain.dense_reconciled` and
`explain.fused_reconciled`; a row whose carried provenance the index does not
reproduce fails on that axis.

`doctor` is the composite read-only diagnosis: it fuses the activation-catalog
listing with per-track serve-time resolution and reports a machine-readable
verdict (serve_ready, all_resolvable, per-track resolver_ok). It exits 0 when the
diagnosis completes (read the JSON verdict fields for the health conclusion) and
uses the standard transport/usage/remote/protocol exit codes only for failures
to *reach* a verdict.

`metrics` scrapes every counter, gauge and histogram the daemon has aggregated
since it started (route latency, outcomes and interruptions, socket admission,
queue wait and in-flight slots, response bytes, examined candidates, caches,
provider retries and failures, generation disk bytes, the process resident
set, the maintenance timer, boot inventory) over the control socket;
`--output prometheus` emits the text exposition format for a scraper.

Operator scrape path: the daemon listens on a private Unix socket, not an
HTTP port, so a Prometheus server does not scrape it directly. Run this
command from the daemon's user on a schedule and write its output where the
node_exporter textfile collector reads it, e.g.

  quanta-index-searchctl --state-root /var/lib/quanta-index metrics \
    --output prometheus > /var/lib/node_exporter/textfile/quanta_index.prom.tmp \
    && mv /var/lib/node_exporter/textfile/quanta_index.prom.tmp \
          /var/lib/node_exporter/textfile/quanta_index.prom

(node_exporter must run with `--collector.textfile.directory` pointing at
that directory; the temporary-file-then-rename keeps a half-written scrape
from being read). Every metric is process-wide and label-free by design:
no repository, generation or query text ever becomes a label.

`quarantine list` prints what the daemon has set aside as it cannot trust it
(a generation directory with an unreadable or foreign identity, a RepoMap file
that does not decode), each line in the form `quarantine discard` takes back.
A discard names one entry exactly as listed; the daemon re-checks before it
removes anything and refuses a stale or invented entry typed.
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

/// The typed refusal every non-metrics renderer gives `--output prometheus`.
///
/// `ParsedCommand::parse` refuses the combination before any socket is
/// touched; the renderers keep the arm so the refusal is the same wherever
/// the mode arrives.
fn prometheus_is_metrics_only(command: &str) -> CliError {
    CliError::usage(format!(
        "`--output prometheus` renders only `metrics`, not `{command}`"
    ))
}

#[cfg(test)]
mod tests;
