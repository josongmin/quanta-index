#![expect(
    clippy::indexing_slicing,
    reason = "doctor tests index serde_json::Value by key/position to assert the stable JSON contract this crate constructs; an out-of-range index is a legitimate test failure"
)]

use std::collections::BTreeMap;
use std::fs;
use std::process::Command;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use quanta_index_contract::ipc::{
    CurrentGenerationRequest, GenerationSnapshot, GenerationStatusReport, GenerationStatusRequest,
    SearchPlaneControlIpcRequest, SearchPlaneControlIpcRequestEnvelope,
    SearchPlaneControlIpcResponse, SearchPlaneControlIpcResponseEnvelope, SearchPlaneTrackKind,
    TrackReadinessRecord,
};
use quanta_index_contract::lex::ExplanationRow;
use quanta_index_contract::{
    EngineTouched, GenerationPin, HybridSeedCandidate, HybridSeedLane, HybridSeedQueryResponse,
    LexicalCandidate, ManifestGeneration, PlannerStage, PlannerTraceEntry, QueryResultWindowV1,
    RepoId, RepoMapDocType, RepoMapEntryDto, RepoMapExactnessSummary, RepoMapFocusSubjectDto,
    RepoMapGraphCoverageClass, RepoMapItemIndexAvailability, RepoMapQueryResponse,
    RepoMapRedactionState, RepoMapSnapshotMeta, RepoRelativePath, RevisionId, SearchExplanation,
    SearchPlaneExplainQueryResponse, SearchPlaneIpcError, SearchPlaneQueryIpcRequest,
    SearchPlaneQueryIpcRequestEnvelope, SearchPlaneQueryIpcResponse,
    SearchPlaneQueryIpcResponseEnvelope, SemanticQueryResponse, TextQueryResponse, TextQuerySyntax,
};
use quanta_index_ipc::{IpcDispatcher, UdsServer};
use tempfile::tempdir;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum SmokeScenario {
    LexicalJson,
    ExplainPretty,
    SemanticJson,
    HybridSeedPretty,
    RepoMapPretty,
}

struct ScenarioDispatcher {
    scenario: SmokeScenario,
}

static NEXT_SOCKET_ID: AtomicU64 = AtomicU64::new(0);

impl IpcDispatcher<SearchPlaneQueryIpcRequest, SearchPlaneQueryIpcResponse> for ScenarioDispatcher {
    fn dispatch(&self, request: SearchPlaneQueryIpcRequest) -> SearchPlaneQueryIpcResponse {
        match self.scenario {
            SmokeScenario::LexicalJson => dispatch_lexical_request(request),
            SmokeScenario::ExplainPretty => dispatch_explain_request(request),
            SmokeScenario::SemanticJson => dispatch_semantic_request(request),
            SmokeScenario::HybridSeedPretty => dispatch_hybrid_seed_request(request),
            SmokeScenario::RepoMapPretty => dispatch_repomap_request(request),
        }
    }
}

#[test]
fn lexical_json_roundtrip() {
    let result = lexical_json_roundtrip_impl();
    assert!(result.is_ok(), "{result:?}");
}

#[test]
fn explain_pretty_roundtrip() {
    let result = explain_pretty_roundtrip_impl();
    assert!(result.is_ok(), "{result:?}");
}

#[test]
fn semantic_query_text_json_roundtrip() {
    let result = semantic_query_text_json_roundtrip_impl();
    assert!(result.is_ok(), "{result:?}");
}

#[test]
fn hybrid_query_text_pretty_roundtrip() {
    let result = hybrid_query_text_pretty_roundtrip_impl();
    assert!(result.is_ok(), "{result:?}");
}

#[test]
fn repomap_pretty_roundtrip() {
    let result = repomap_pretty_roundtrip_impl();
    assert!(result.is_ok(), "{result:?}");
}

#[test]
fn control_subcommands_are_not_exposed() {
    let result = control_subcommands_are_not_exposed_impl();
    assert!(result.is_ok(), "{result:?}");
}

#[test]
fn semantic_scope_query_requires_scope_top_k() {
    let result = semantic_scope_query_requires_scope_top_k_impl();
    assert!(result.is_ok(), "{result:?}");
}

#[test]
fn semantic_scope_top_k_without_scope_query_is_rejected() {
    let result = semantic_scope_top_k_without_scope_query_is_rejected_impl();
    assert!(result.is_ok(), "{result:?}");
}

#[test]
fn semantic_with_scope_query_and_scope_top_k_parses_successfully() {
    let result = semantic_with_scope_query_and_scope_top_k_parses_successfully_impl();
    assert!(result.is_ok(), "{result:?}");
}

fn lexical_json_roundtrip_impl() -> Result<(), Box<dyn std::error::Error>> {
    let socket_path = unique_socket_path();
    let shutdown = start_server(&socket_path, SmokeScenario::LexicalJson)?;
    let output = Command::new(env!("CARGO_BIN_EXE_quanta-index-searchctl"))
        .arg("lexical")
        .arg("--socket")
        .arg(&socket_path)
        .arg("--output")
        .arg("json")
        .arg("--repo-id")
        .arg("repo-1")
        .arg("--revision-id")
        .arg("rev-1")
        .arg("--manifest-generation")
        .arg("11")
        .arg("--syntax")
        .arg("native")
        .arg("--query-text")
        .arg("fn main")
        .arg("--top-k")
        .arg("5")
        .output()?;
    shutdown.trigger();
    if !output.status.success() {
        return Err(String::from_utf8_lossy(&output.stderr).into_owned().into());
    }
    let stdout = String::from_utf8(output.stdout)?;
    if !stdout.contains("\"kind\": \"Text\"") {
        return Err(format!("missing text response kind in stdout: {stdout}").into());
    }
    if !stdout.contains("\"candidate_id\": \"cand-1\"") {
        return Err(format!("missing lexical candidate in stdout: {stdout}").into());
    }
    Ok(())
}

fn explain_pretty_roundtrip_impl() -> Result<(), Box<dyn std::error::Error>> {
    let tempdir = tempdir()?;
    let socket_path = unique_socket_path();
    let candidate_path = tempdir.path().join("candidate.json");
    let candidate = stub_candidate(stub_generation());
    fs::write(&candidate_path, serde_json::to_vec_pretty(&candidate)?)?;
    let shutdown = start_server(&socket_path, SmokeScenario::ExplainPretty)?;
    let output = Command::new(env!("CARGO_BIN_EXE_quanta-index-searchctl"))
        .arg("explain")
        .arg("--socket")
        .arg(&socket_path)
        .arg("--repo-id")
        .arg("repo-1")
        .arg("--revision-id")
        .arg("rev-1")
        .arg("--manifest-generation")
        .arg("11")
        .arg("--candidate-json")
        .arg(&candidate_path)
        .output()?;
    shutdown.trigger();
    if !output.status.success() {
        return Err(String::from_utf8_lossy(&output.stderr).into_owned().into());
    }
    let stdout = String::from_utf8(output.stdout)?;
    if !stdout.contains("kind: explain") {
        return Err(format!("missing explain kind in stdout: {stdout}").into());
    }
    if !stdout.contains("summary: fused lexical explanation") {
        return Err(format!("missing explain summary in stdout: {stdout}").into());
    }
    if !stdout.contains("planner_trace:") {
        return Err(format!("missing planner trace in stdout: {stdout}").into());
    }
    Ok(())
}

fn semantic_query_text_json_roundtrip_impl() -> Result<(), Box<dyn std::error::Error>> {
    let socket_path = unique_socket_path();
    let shutdown = start_server(&socket_path, SmokeScenario::SemanticJson)?;
    let output = Command::new(env!("CARGO_BIN_EXE_quanta-index-searchctl"))
        .arg("semantic")
        .arg("--socket")
        .arg(&socket_path)
        .arg("--output")
        .arg("json")
        .arg("--repo-id")
        .arg("repo-1")
        .arg("--revision-id")
        .arg("rev-1")
        .arg("--manifest-generation")
        .arg("11")
        .arg("--query-text")
        .arg("0.25 0.5 -0.75")
        .arg("--top-k")
        .arg("10")
        .arg("--scope-query")
        .arg("repo:repo-1 file:src/lib.rs")
        .arg("--scope-syntax")
        .arg("sourcegraph")
        .arg("--scope-top-k")
        .arg("4")
        .output()?;
    shutdown.trigger();
    if !output.status.success() {
        return Err(String::from_utf8_lossy(&output.stderr).into_owned().into());
    }
    let stdout = String::from_utf8(output.stdout)?;
    if !stdout.contains("\"kind\": \"Semantic\"") {
        return Err(format!("missing semantic response kind in stdout: {stdout}").into());
    }
    if !stdout.contains("\"candidate_id\": \"cand-1\"") {
        return Err(format!("missing semantic candidate in stdout: {stdout}").into());
    }
    Ok(())
}

fn hybrid_query_text_pretty_roundtrip_impl() -> Result<(), Box<dyn std::error::Error>> {
    let socket_path = unique_socket_path();
    let shutdown = start_server(&socket_path, SmokeScenario::HybridSeedPretty)?;
    let output = Command::new(env!("CARGO_BIN_EXE_quanta-index-searchctl"))
        .arg("hybrid-seed")
        .arg("--socket")
        .arg(&socket_path)
        .arg("--repo-id")
        .arg("repo-1")
        .arg("--revision-id")
        .arg("rev-1")
        .arg("--manifest-generation")
        .arg("11")
        .arg("--lexical-query")
        .arg("symbol:RepoMapOwner")
        .arg("--lexical-syntax")
        .arg("sourcegraph")
        .arg("--semantic-query")
        .arg("0.25 0.5 -0.75")
        .arg("--top-k")
        .arg("3")
        .output()?;
    shutdown.trigger();
    if !output.status.success() {
        return Err(String::from_utf8_lossy(&output.stderr).into_owned().into());
    }
    let stdout = String::from_utf8(output.stdout)?;
    if !stdout.contains("kind: hybrid-seed") {
        return Err(format!("missing hybrid-seed kind in stdout: {stdout}").into());
    }
    if !stdout.contains("summary: hybrid seed explanation") {
        return Err(format!("missing hybrid-seed summary in stdout: {stdout}").into());
    }
    Ok(())
}

fn repomap_pretty_roundtrip_impl() -> Result<(), Box<dyn std::error::Error>> {
    let socket_path = unique_socket_path();
    let shutdown = start_server(&socket_path, SmokeScenario::RepoMapPretty)?;
    let output = Command::new(env!("CARGO_BIN_EXE_quanta-index-searchctl"))
        .arg("repomap")
        .arg("--socket")
        .arg(&socket_path)
        .arg("--repo-id")
        .arg("repo-1")
        .arg("--revision-id")
        .arg("rev-1")
        .arg("--manifest-generation")
        .arg("11")
        .arg("--query-text")
        .arg("repo map focus")
        .arg("--top-k")
        .arg("5")
        .arg("--token-budget")
        .arg("2048")
        .arg("--focus-subject")
        .arg("subject-repomap:symbol")
        .output()?;
    shutdown.trigger();
    if !output.status.success() {
        return Err(String::from_utf8_lossy(&output.stderr).into_owned().into());
    }
    let stdout = String::from_utf8(output.stdout)?;
    if !stdout.contains("kind: repomap") {
        return Err(format!("missing repomap kind in stdout: {stdout}").into());
    }
    if !stdout.contains("subject_identity=entry::ident") {
        return Err(format!("missing repomap entry in stdout: {stdout}").into());
    }
    Ok(())
}

fn control_subcommands_are_not_exposed_impl() -> Result<(), Box<dyn std::error::Error>> {
    let output = Command::new(env!("CARGO_BIN_EXE_quanta-index-searchctl"))
        .arg("repomap-activate")
        .output()?;
    if output.status.code() != Some(2) {
        return Err(format!("unexpected exit code: {:?}", output.status.code()).into());
    }
    let stderr = String::from_utf8(output.stderr)?;
    if !stderr.contains("unknown subcommand") {
        return Err(format!("missing unknown-subcommand error in stderr: {stderr}").into());
    }
    Ok(())
}

fn semantic_scope_query_requires_scope_top_k_impl() -> Result<(), Box<dyn std::error::Error>> {
    let socket_path = unique_socket_path();
    let output = Command::new(env!("CARGO_BIN_EXE_quanta-index-searchctl"))
        .arg("semantic")
        .arg("--socket")
        .arg(&socket_path)
        .arg("--repo-id")
        .arg("repo-1")
        .arg("--revision-id")
        .arg("rev-1")
        .arg("--manifest-generation")
        .arg("11")
        .arg("--query-text")
        .arg("foo")
        .arg("--top-k")
        .arg("10")
        .arg("--scope-query")
        .arg("foo")
        .arg("--scope-syntax")
        .arg("native")
        .output()?;
    if output.status.code() != Some(2) {
        return Err(format!(
            "expected usage exit code 2, got {:?} (stderr: {})",
            output.status.code(),
            String::from_utf8_lossy(&output.stderr)
        )
        .into());
    }
    let stderr = String::from_utf8(output.stderr)?;
    if !stderr.contains("scope-top-k") {
        return Err(format!("missing scope-top-k in stderr: {stderr}").into());
    }
    Ok(())
}

fn semantic_scope_top_k_without_scope_query_is_rejected_impl()
-> Result<(), Box<dyn std::error::Error>> {
    let socket_path = unique_socket_path();
    let output = Command::new(env!("CARGO_BIN_EXE_quanta-index-searchctl"))
        .arg("semantic")
        .arg("--socket")
        .arg(&socket_path)
        .arg("--repo-id")
        .arg("repo-1")
        .arg("--revision-id")
        .arg("rev-1")
        .arg("--manifest-generation")
        .arg("11")
        .arg("--query-text")
        .arg("foo")
        .arg("--top-k")
        .arg("10")
        .arg("--scope-top-k")
        .arg("20")
        .output()?;
    if output.status.code() != Some(2) {
        return Err(format!(
            "expected usage exit code 2, got {:?} (stderr: {})",
            output.status.code(),
            String::from_utf8_lossy(&output.stderr)
        )
        .into());
    }
    let stderr = String::from_utf8(output.stderr)?;
    if !stderr.contains("--scope-top-k") {
        return Err(format!("missing --scope-top-k in stderr: {stderr}").into());
    }
    Ok(())
}

fn semantic_with_scope_query_and_scope_top_k_parses_successfully_impl()
-> Result<(), Box<dyn std::error::Error>> {
    // Point at a socket path that definitely does not exist. If parsing succeeds, the CLI
    // proceeds to IPC and fails with EXIT_TRANSPORT (1); if parsing fails, we get
    // EXIT_USAGE (2). The assertion below distinguishes the two.
    let socket_path = unique_socket_path();
    let output = Command::new(env!("CARGO_BIN_EXE_quanta-index-searchctl"))
        .arg("semantic")
        .arg("--socket")
        .arg(&socket_path)
        .arg("--repo-id")
        .arg("repo-1")
        .arg("--revision-id")
        .arg("rev-1")
        .arg("--manifest-generation")
        .arg("11")
        .arg("--query-text")
        .arg("embedding query")
        .arg("--top-k")
        .arg("10")
        .arg("--scope-query")
        .arg("foo")
        .arg("--scope-syntax")
        .arg("native")
        .arg("--scope-top-k")
        .arg("20")
        .output()?;
    if output.status.code() == Some(2) {
        return Err(format!(
            "parsing should succeed but got usage exit code (stderr: {})",
            String::from_utf8_lossy(&output.stderr)
        )
        .into());
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// J7Q-05: control-plane `doctor` smoke. `doctor` fuses the activation-catalog
// listing (`GenerationStatus`) with per-track serve-time resolution
// (`CurrentGeneration`) over the CONTROL socket. The mock below drives each
// classification arm of `build_doctor_report`: resolved, divergent, NOT_READY,
// and the benign empty-catalog case.
// ---------------------------------------------------------------------------

const DOCTOR_REPO: &str = "repo-doctor";
const DOCTOR_REV: &str = "rev-doctor";
const DOCTOR_DIGEST: &str = "lex-digest-11";

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ControlScenario {
    ResolvedLexicalTrack,
    DivergentResolver,
    NotReadyResolver,
    EmptyTracks,
}

struct ControlScenarioDispatcher {
    scenario: ControlScenario,
}

impl ControlScenarioDispatcher {
    fn listed_track(&self) -> Option<TrackReadinessRecord> {
        match self.scenario {
            ControlScenario::EmptyTracks => None,
            ControlScenario::ResolvedLexicalTrack
            | ControlScenario::DivergentResolver
            | ControlScenario::NotReadyResolver => Some(TrackReadinessRecord {
                track: SearchPlaneTrackKind::Lexical,
                manifest_generation: ManifestGeneration::new(11),
                manifest_digest: DOCTOR_DIGEST.to_string(),
            }),
        }
    }

    fn generation_status(&self, req: &GenerationStatusRequest) -> SearchPlaneControlIpcResponse {
        if req.repo_id != RepoId::new(DOCTOR_REPO) || req.revision_id != RevisionId::new(DOCTOR_REV)
        {
            return control_error_response(
                "TEST_BAD_REPO_REV",
                format!("unexpected status request: {req:?}"),
            );
        }
        SearchPlaneControlIpcResponse::GenerationStatusReport(GenerationStatusReport {
            repo_id: req.repo_id.clone(),
            revision_id: req.revision_id.clone(),
            tracks: self.listed_track().into_iter().collect(),
        })
    }

    fn current_generation(&self, req: &CurrentGenerationRequest) -> SearchPlaneControlIpcResponse {
        match self.scenario {
            ControlScenario::ResolvedLexicalTrack => {
                SearchPlaneControlIpcResponse::CurrentGenerationSnapshot(GenerationSnapshot {
                    repo_id: req.repo_id.clone(),
                    revision_id: req.revision_id.clone(),
                    track: req.track,
                    manifest_generation: ManifestGeneration::new(11),
                    manifest_digest: DOCTOR_DIGEST.to_string(),
                })
            }
            ControlScenario::DivergentResolver => {
                SearchPlaneControlIpcResponse::CurrentGenerationSnapshot(GenerationSnapshot {
                    repo_id: req.repo_id.clone(),
                    revision_id: req.revision_id.clone(),
                    track: req.track,
                    manifest_generation: ManifestGeneration::new(11),
                    manifest_digest: "DIVERGENT-digest".to_string(),
                })
            }
            ControlScenario::NotReadyResolver => {
                control_error_response("NOT_READY", "resolver not ready for the listed track")
            }
            ControlScenario::EmptyTracks => {
                control_error_response("NOT_READY", "no activated generation")
            }
        }
    }
}

impl IpcDispatcher<SearchPlaneControlIpcRequest, SearchPlaneControlIpcResponse>
    for ControlScenarioDispatcher
{
    fn dispatch(&self, request: SearchPlaneControlIpcRequest) -> SearchPlaneControlIpcResponse {
        match request {
            SearchPlaneControlIpcRequest::GenerationStatus(req) => self.generation_status(&req),
            SearchPlaneControlIpcRequest::CurrentGeneration(req) => self.current_generation(&req),
            other @ (SearchPlaneControlIpcRequest::ActivateSearchCorpusGenerationCas(_)
            | SearchPlaneControlIpcRequest::RollbackSearchCorpusGenerationCas(_)
            | SearchPlaneControlIpcRequest::RepoMapActivate(_)) => control_error_response(
                "TEST_UNEXPECTED_CONTROL_REQUEST",
                format!("doctor mock received unexpected control request: {other:?}"),
            ),
        }
    }
}

fn control_error_response(code: &str, message: impl Into<String>) -> SearchPlaneControlIpcResponse {
    SearchPlaneControlIpcResponse::Error(SearchPlaneIpcError {
        code: code.to_string(),
        message: message.into(),
        repair: None,
    })
}

fn start_control_server(
    socket_path: &std::path::Path,
    scenario: ControlScenario,
) -> Result<quanta_index_ipc::ShutdownHandle, Box<dyn std::error::Error>> {
    let server = UdsServer::bind(socket_path)?;
    let shutdown = server.shutdown_handle();
    let dispatcher = Arc::new(ControlScenarioDispatcher { scenario });
    let _server_thread = std::thread::spawn(move || {
        match server.run::<
            SearchPlaneControlIpcRequestEnvelope,
            SearchPlaneControlIpcRequest,
            SearchPlaneControlIpcResponseEnvelope,
            SearchPlaneControlIpcResponse,
            ControlScenarioDispatcher,
        >(&dispatcher, Duration::from_millis(5))
        {
            Ok(()) | Err(_) => {}
        }
    });
    for _attempt in 0..20 {
        if socket_path.exists() {
            break;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    Ok(shutdown)
}

/// Run `doctor --output json` against a control mock and return the decoded JSON.
///
/// `--socket <dir>/query.sock` makes the SDK derive the control socket as the
/// sibling `<dir>/control.sock`, which is where the mock binds; the query socket
/// is never connected (doctor is control-plane only).
fn run_doctor_json(
    scenario: ControlScenario,
) -> Result<serde_json::Value, Box<dyn std::error::Error>> {
    let dir = tempdir()?;
    let query_socket = dir.path().join("query.sock");
    let control_socket = dir.path().join("control.sock");
    let shutdown = start_control_server(&control_socket, scenario)?;
    let output = Command::new(env!("CARGO_BIN_EXE_quanta-index-searchctl"))
        .arg("doctor")
        .arg("--socket")
        .arg(&query_socket)
        .arg("--output")
        .arg("json")
        .arg("--repo-id")
        .arg(DOCTOR_REPO)
        .arg("--revision-id")
        .arg(DOCTOR_REV)
        .output()?;
    shutdown.trigger();
    if !output.status.success() {
        return Err(format!(
            "doctor exited non-zero ({:?}): {}",
            output.status.code(),
            String::from_utf8_lossy(&output.stderr)
        )
        .into());
    }
    let stdout = String::from_utf8(output.stdout)?;
    Ok(serde_json::from_str::<serde_json::Value>(&stdout)?)
}

#[test]
fn doctor_json_reports_resolved_track() {
    let result = doctor_json_reports_resolved_track_impl();
    assert!(result.is_ok(), "{result:?}");
}

fn doctor_json_reports_resolved_track_impl() -> Result<(), Box<dyn std::error::Error>> {
    let value = run_doctor_json(ControlScenario::ResolvedLexicalTrack)?;
    if value["kind"].as_str() != Some("doctor") {
        return Err(format!("missing doctor kind: {value}").into());
    }
    if value["serve_ready"].as_bool() != Some(true) {
        return Err(format!("expected serve_ready=true: {value}").into());
    }
    if value["all_resolvable"].as_bool() != Some(true) {
        return Err(format!("expected all_resolvable=true: {value}").into());
    }
    if value["track_count"].as_u64() != Some(1) {
        return Err(format!("expected track_count=1: {value}").into());
    }
    let track = &value["tracks"][0];
    if track["track"].as_str() != Some("Lexical") {
        return Err(format!("expected Lexical track: {value}").into());
    }
    if track["manifest_digest"].as_str() != Some(DOCTOR_DIGEST) {
        return Err(format!("expected catalog digest: {value}").into());
    }
    if track["resolver_ok"].as_bool() != Some(true) {
        return Err(format!("expected resolver_ok=true: {value}").into());
    }
    Ok(())
}

#[test]
fn doctor_flags_resolver_divergence() {
    let result = doctor_flags_resolver_divergence_impl();
    assert!(result.is_ok(), "{result:?}");
}

fn doctor_flags_resolver_divergence_impl() -> Result<(), Box<dyn std::error::Error>> {
    // A divergent serve-time digest is a recorded finding, NOT an abort: doctor
    // still exits 0 (the diagnosis completed) and surfaces the inconsistency.
    let value = run_doctor_json(ControlScenario::DivergentResolver)?;
    if value["all_resolvable"].as_bool() != Some(false) {
        return Err(format!("expected all_resolvable=false on divergence: {value}").into());
    }
    if value["serve_ready"].as_bool() != Some(false) {
        return Err(format!("expected serve_ready=false on divergence: {value}").into());
    }
    let track = &value["tracks"][0];
    if track["resolver_ok"].as_bool() != Some(false) {
        return Err(format!("expected resolver_ok=false on divergence: {value}").into());
    }
    match track["resolver_note"].as_str() {
        Some(note) if note.contains("divergence") => Ok(()),
        _ => Err(format!("expected divergence note: {value}").into()),
    }
}

#[test]
fn doctor_flags_not_ready_resolver() {
    let result = doctor_flags_not_ready_resolver_impl();
    assert!(result.is_ok(), "{result:?}");
}

fn doctor_flags_not_ready_resolver_impl() -> Result<(), Box<dyn std::error::Error>> {
    // A typed NOT_READY for a *listed* track is a finding, not a fatal remote
    // error: doctor exits 0 and marks the track unresolvable.
    let value = run_doctor_json(ControlScenario::NotReadyResolver)?;
    if value["track_count"].as_u64() != Some(1) {
        return Err(format!("expected track_count=1: {value}").into());
    }
    if value["all_resolvable"].as_bool() != Some(false) {
        return Err(format!("expected all_resolvable=false on NOT_READY: {value}").into());
    }
    let track = &value["tracks"][0];
    if track["resolver_ok"].as_bool() != Some(false) {
        return Err(format!("expected resolver_ok=false on NOT_READY: {value}").into());
    }
    match track["resolver_note"].as_str() {
        Some(note) if note.contains("NOT_READY") => Ok(()),
        _ => Err(format!("expected NOT_READY note: {value}").into()),
    }
}

#[test]
fn doctor_empty_catalog_is_benign() {
    let result = doctor_empty_catalog_is_benign_impl();
    assert!(result.is_ok(), "{result:?}");
}

fn doctor_empty_catalog_is_benign_impl() -> Result<(), Box<dyn std::error::Error>> {
    // Nothing activated yet is a legitimate state: serve_ready=false (nothing to
    // serve) but all_resolvable=true (no inconsistency). doctor exits 0.
    let value = run_doctor_json(ControlScenario::EmptyTracks)?;
    if value["track_count"].as_u64() != Some(0) {
        return Err(format!("expected track_count=0: {value}").into());
    }
    if value["serve_ready"].as_bool() != Some(false) {
        return Err(format!("expected serve_ready=false on empty catalog: {value}").into());
    }
    if value["all_resolvable"].as_bool() != Some(true) {
        return Err(format!("expected all_resolvable=true on empty catalog: {value}").into());
    }
    Ok(())
}

#[test]
fn doctor_missing_repo_id_is_usage_error() {
    let result = doctor_missing_repo_id_is_usage_error_impl();
    assert!(result.is_ok(), "{result:?}");
}

fn doctor_missing_repo_id_is_usage_error_impl() -> Result<(), Box<dyn std::error::Error>> {
    let output = Command::new(env!("CARGO_BIN_EXE_quanta-index-searchctl"))
        .arg("doctor")
        .arg("--revision-id")
        .arg("rev-doctor")
        .output()?;
    if output.status.code() != Some(2) {
        return Err(format!(
            "expected usage exit code 2, got {:?} (stderr: {})",
            output.status.code(),
            String::from_utf8_lossy(&output.stderr)
        )
        .into());
    }
    let stderr = String::from_utf8(output.stderr)?;
    if !stderr.contains("--repo-id") {
        return Err(format!("missing --repo-id in stderr: {stderr}").into());
    }
    Ok(())
}

fn start_server(
    socket_path: &std::path::Path,
    scenario: SmokeScenario,
) -> Result<quanta_index_ipc::ShutdownHandle, Box<dyn std::error::Error>> {
    let server = UdsServer::bind(socket_path)?;
    let shutdown = server.shutdown_handle();
    let dispatcher = Arc::new(ScenarioDispatcher { scenario });
    let _server_thread = std::thread::spawn(move || {
        match server.run::<
            SearchPlaneQueryIpcRequestEnvelope,
            SearchPlaneQueryIpcRequest,
            SearchPlaneQueryIpcResponseEnvelope,
            SearchPlaneQueryIpcResponse,
            ScenarioDispatcher,
        >(&dispatcher, Duration::from_millis(5))
        {
            Ok(()) | Err(_) => {}
        }
    });
    for _attempt in 0..20 {
        if socket_path.exists() {
            break;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    Ok(shutdown)
}

fn unique_socket_path() -> std::path::PathBuf {
    let pid = std::process::id();
    let nanos = match std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH) {
        Ok(duration) => duration.as_nanos(),
        Err(_err) => 0,
    };
    let sequence = NEXT_SOCKET_ID.fetch_add(1, Ordering::Relaxed);
    std::env::temp_dir().join(format!("qi-searchctl-test-{pid}-{nanos}-{sequence}.sock"))
}

fn dispatch_lexical_request(request: SearchPlaneQueryIpcRequest) -> SearchPlaneQueryIpcResponse {
    let SearchPlaneQueryIpcRequest::Text(payload) = request else {
        return error_response(
            "TEST_UNEXPECTED_REQUEST",
            format!("expected lexical request, got {request:?}"),
        );
    };
    let Some(generation) = payload.generation else {
        return error_response(
            "TEST_MISSING_GENERATION",
            "lexical request must carry generation",
        );
    };
    SearchPlaneQueryIpcResponse::Text(TextQueryResponse {
        generation: generation.clone(),
        results: vec![stub_candidate(generation)],
        window: QueryResultWindowV1::exact(1),
        file_owner_rows: None,
    })
}

fn dispatch_explain_request(request: SearchPlaneQueryIpcRequest) -> SearchPlaneQueryIpcResponse {
    let SearchPlaneQueryIpcRequest::Explain(payload) = request else {
        return error_response(
            "TEST_UNEXPECTED_REQUEST",
            format!("expected explain request, got {request:?}"),
        );
    };
    SearchPlaneQueryIpcResponse::Explain(SearchPlaneExplainQueryResponse {
        generation: payload.generation,
        explanation: stub_explanation(
            "fused lexical explanation",
            vec![EngineTouched::Lexical, EngineTouched::Semantic],
        ),
    })
}

fn dispatch_semantic_request(request: SearchPlaneQueryIpcRequest) -> SearchPlaneQueryIpcResponse {
    let SearchPlaneQueryIpcRequest::Semantic(payload) = request else {
        return error_response(
            "TEST_UNEXPECTED_REQUEST",
            format!("expected semantic request, got {request:?}"),
        );
    };
    let expected_generation = stub_generation();
    if payload.generation.as_ref() != Some(&expected_generation) {
        return error_response(
            "TEST_BAD_GENERATION",
            format!("unexpected semantic generation: {:?}", payload.generation),
        );
    }
    if payload.query_text.as_str() != "0.25 0.5 -0.75" {
        return error_response(
            "TEST_BAD_QUERY_TEXT",
            format!("unexpected semantic query_text: {:?}", payload.query_text),
        );
    }
    let Some(scope) = payload.lexical_scope.as_ref() else {
        return error_response(
            "TEST_MISSING_SCOPE",
            "semantic request should carry lexical scope",
        );
    };
    if scope.syntax != TextQuerySyntax::Sourcegraph
        || scope.query_text != "repo:repo-1 file:src/lib.rs"
        || scope.top_k != 4
        || scope.generation.as_ref() != Some(&expected_generation)
    {
        return error_response(
            "TEST_BAD_SCOPE",
            format!("unexpected semantic lexical scope: {scope:?}"),
        );
    }
    if payload.top_k != 10 {
        return error_response(
            "TEST_BAD_TOP_K",
            format!("unexpected semantic top_k: {}", payload.top_k),
        );
    }
    SearchPlaneQueryIpcResponse::Semantic(SemanticQueryResponse {
        generation: expected_generation.clone(),
        results: vec![stub_candidate(expected_generation)],
        window: QueryResultWindowV1::exact(1),
        explanation: stub_explanation("semantic explanation", vec![EngineTouched::Semantic]),
    })
}

fn dispatch_hybrid_seed_request(
    request: SearchPlaneQueryIpcRequest,
) -> SearchPlaneQueryIpcResponse {
    let SearchPlaneQueryIpcRequest::HybridSeed(payload) = request else {
        return error_response(
            "TEST_UNEXPECTED_REQUEST",
            format!("expected hybrid-seed request, got {request:?}"),
        );
    };
    let expected_generation = stub_generation();
    if payload.generation.as_ref() != Some(&expected_generation) {
        return error_response(
            "TEST_BAD_GENERATION",
            format!("unexpected hybrid generation: {:?}", payload.generation),
        );
    }
    if payload.text_query.syntax != TextQuerySyntax::Sourcegraph
        || payload.text_query.query_text != "symbol:RepoMapOwner"
        || payload.text_query.top_k != 3
        || payload.text_query.generation.as_ref() != Some(&expected_generation)
    {
        return error_response(
            "TEST_BAD_TEXT_QUERY",
            format!("unexpected hybrid text query: {:?}", payload.text_query),
        );
    }
    if payload.semantic_query_text.as_str() != "0.25 0.5 -0.75" {
        return error_response(
            "TEST_BAD_SEMANTIC_QUERY_TEXT",
            format!(
                "unexpected hybrid semantic query text: {:?}",
                payload.semantic_query_text
            ),
        );
    }
    if payload.top_k != 3 {
        return error_response(
            "TEST_BAD_TOP_K",
            format!("unexpected hybrid top_k: {}", payload.top_k),
        );
    }
    SearchPlaneQueryIpcResponse::HybridSeed(HybridSeedQueryResponse {
        generation: expected_generation.clone(),
        manifest_digest: "manifest-digest-9".to_string(),
        seed_candidates: vec![HybridSeedCandidate {
            candidate: stub_candidate(expected_generation),
            seed_rank: 1,
            lexical_rank: Some(1),
            lexical_score_raw: Some(1.0),
            semantic_rank: Some(1),
            semantic_score_raw: Some(0.5),
            source_lanes: vec![HybridSeedLane::Lexical, HybridSeedLane::Semantic],
        }],
        seed_candidates_v2: None,
        window: QueryResultWindowV1::exact(1),
        explanation: stub_explanation(
            "hybrid seed explanation",
            vec![EngineTouched::Lexical, EngineTouched::Semantic],
        ),
    })
}

fn dispatch_repomap_request(request: SearchPlaneQueryIpcRequest) -> SearchPlaneQueryIpcResponse {
    let SearchPlaneQueryIpcRequest::RepoMapQuery(payload) = request else {
        return error_response(
            "TEST_UNEXPECTED_REQUEST",
            format!("expected repomap request, got {request:?}"),
        );
    };
    if payload.repo_id != RepoId::new("repo-1")
        || payload.revision_id != RevisionId::new("rev-1")
        || payload.manifest_generation != ManifestGeneration::new(11)
        || payload.query_text != "repo map focus"
        || payload.top_k != 5
        || payload.token_budget != 2048
        || payload.focus_subjects != vec![stub_repomap_focus_subject()]
    {
        return error_response(
            "TEST_BAD_REPOMAP_REQUEST",
            format!("unexpected repomap request: {payload:?}"),
        );
    }
    SearchPlaneQueryIpcResponse::RepoMapQuery(stub_repomap_response())
}

fn error_response(code: &str, message: impl Into<String>) -> SearchPlaneQueryIpcResponse {
    SearchPlaneQueryIpcResponse::Error(SearchPlaneIpcError {
        code: code.to_string(),
        message: message.into(),
        repair: None,
    })
}

fn stub_generation() -> GenerationPin {
    GenerationPin::new(
        RepoId::new("repo-1"),
        RevisionId::new("rev-1"),
        ManifestGeneration::new(11),
    )
}

fn stub_candidate(generation: GenerationPin) -> LexicalCandidate {
    LexicalCandidate {
        candidate_id: "cand-1".to_string(),
        repo_id: generation.repo_id,
        revision_id: generation.revision_id,
        manifest_generation: generation.manifest_generation,
        repo_relative_path: RepoRelativePath::new("src/lib.rs"),
        start_line: 10,
        end_line: 14,
        score: 0.91,
        snippet: "fn main() {\n    println!(\"hi\");\n}".to_string(),
        snippet_hit_offset: None,
        highlights: Vec::new(),
    }
}

fn stub_explanation(summary: &str, engines_touched: Vec<EngineTouched>) -> SearchExplanation {
    SearchExplanation {
        planner_trace: vec![PlannerTraceEntry {
            stage: PlannerStage::Merge,
            detail: "rrf fused lexical candidates".to_string(),
        }],
        engines_touched,
        early_stop_reason: None,
        contributions: vec![ExplanationRow {
            signal_name: "bm25".into(),
            signal_value: 0.8,
            weight: 1.0,
            contribution: 0.8,
        }],
        ranker_weights_hash: [7_u8; 32],
        strategy: "rrf".to_string(),
        summary: summary.to_string(),
    }
}

fn stub_repomap_focus_subject() -> RepoMapFocusSubjectDto {
    RepoMapFocusSubjectDto {
        subject_identity: "subject-repomap".to_string(),
        subject_doc_type: RepoMapDocType::Symbol,
    }
}

fn stub_repomap_snapshot_meta() -> RepoMapSnapshotMeta {
    RepoMapSnapshotMeta {
        snapshot_id: "snap-1".to_string(),
        projection_version: 7,
        authority_digest: "blake3:deadbeef".to_string(),
        item_index_availability: RepoMapItemIndexAvailability::Full,
        graph_coverage_class: RepoMapGraphCoverageClass::Complete,
        exactness_summary: RepoMapExactnessSummary::Exact,
    }
}

fn stub_repomap_entry() -> RepoMapEntryDto {
    RepoMapEntryDto {
        subject_identity: "entry::ident".to_string(),
        subject_doc_type: RepoMapDocType::Symbol,
        subject_kind: "function".to_string(),
        owner_path: "src/lib.rs".to_string(),
        score: 0.875,
        final_score_millis: 875,
        included: true,
        rank: 1,
        importance_score_millis: 500,
        utility_score_millis: 400,
        freshness_score_millis: 300,
        evidence_priority_millis: 200,
        token_budget_hint: 1024,
        contributing_signals: BTreeMap::from([
            ("centrality".to_string(), 100_i64),
            ("recency".to_string(), -3_i64),
        ]),
        projection_evidence_kind: "authoritative".to_string(),
        projection_authority_artifact_id: "art-1".to_string(),
        projection_authority_digest: "blake3:cafebabe".to_string(),
        projection_status: "ok".to_string(),
        redaction_state: RepoMapRedactionState::Unredacted,
    }
}

fn stub_repomap_response() -> RepoMapQueryResponse {
    RepoMapQueryResponse {
        repo_id: RepoId::new("repo-1"),
        revision_id: RevisionId::new("rev-1"),
        manifest_generation: ManifestGeneration::new(11),
        snapshot_meta: stub_repomap_snapshot_meta(),
        entries: vec![stub_repomap_entry()],
        dropped_entries_count: 1,
        drop_reason_codes: vec!["token_budget".to_string()],
        degraded_reason_codes: vec!["partial_authority".to_string()],
    }
}
