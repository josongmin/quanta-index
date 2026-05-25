use std::fs;
use std::process::Command;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use quanta_index_contract::lex::ExplanationRow;
use quanta_index_contract::{
    EngineTouched, GenerationPin, LexicalCandidate, ManifestGeneration, PlannerStage,
    PlannerTraceEntry, RepoId, RepoRelativePath, RevisionId, SearchExplanation,
    SearchPlaneExplainQueryResponse, SearchPlaneIpcError, SearchPlaneQueryIpcRequest,
    SearchPlaneQueryIpcRequestEnvelope, SearchPlaneQueryIpcResponse,
    SearchPlaneQueryIpcResponseEnvelope, TextQueryResponse,
};
use quanta_index_ipc::{IpcDispatcher, UdsServer};
use tempfile::tempdir;

struct StubDispatcher;
static NEXT_SOCKET_ID: AtomicU64 = AtomicU64::new(0);

impl IpcDispatcher<SearchPlaneQueryIpcRequest, SearchPlaneQueryIpcResponse> for StubDispatcher {
    fn dispatch(&self, request: SearchPlaneQueryIpcRequest) -> SearchPlaneQueryIpcResponse {
        match request {
            SearchPlaneQueryIpcRequest::Text(payload) => {
                let Some(generation) = payload.generation else {
                    return error_response(
                        "TEST_MISSING_GENERATION",
                        "lexical request must carry generation",
                    );
                };
                SearchPlaneQueryIpcResponse::Text(TextQueryResponse {
                    generation: generation.clone(),
                    results: vec![stub_candidate(generation)],
                })
            }
            SearchPlaneQueryIpcRequest::Explain(payload) => {
                SearchPlaneQueryIpcResponse::Explain(SearchPlaneExplainQueryResponse {
                    generation: payload.generation,
                    explanation: stub_explanation(),
                })
            }
            other @ (SearchPlaneQueryIpcRequest::Semantic(_)
            | SearchPlaneQueryIpcRequest::Symbol(_)
            | SearchPlaneQueryIpcRequest::Hybrid(_)
            | SearchPlaneQueryIpcRequest::History(_)
            | SearchPlaneQueryIpcRequest::Structural(_)
            | SearchPlaneQueryIpcRequest::Bridge(_)
            | SearchPlaneQueryIpcRequest::RepoMapQuery(_)
            | SearchPlaneQueryIpcRequest::Sourcegraph(_)
            | SearchPlaneQueryIpcRequest::RuntimeMetadata(_)) => error_response(
                "TEST_UNSUPPORTED_REQUEST",
                format!("unexpected request in test: {other:?}"),
            ),
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
fn control_subcommands_are_not_exposed() {
    let result = control_subcommands_are_not_exposed_impl();
    assert!(result.is_ok(), "{result:?}");
}

fn lexical_json_roundtrip_impl() -> Result<(), Box<dyn std::error::Error>> {
    let _tempdir = tempdir()?;
    let socket_path = unique_socket_path();
    let shutdown = start_server(&socket_path)?;
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
    let shutdown = start_server(&socket_path)?;
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

fn start_server(
    socket_path: &std::path::Path,
) -> Result<quanta_index_ipc::ShutdownHandle, Box<dyn std::error::Error>> {
    let server = UdsServer::bind(socket_path)?;
    let shutdown = server.shutdown_handle();
    let dispatcher = Arc::new(StubDispatcher);
    let _server_thread = std::thread::spawn(move || {
        match server.run::<
            SearchPlaneQueryIpcRequestEnvelope,
            SearchPlaneQueryIpcRequest,
            SearchPlaneQueryIpcResponseEnvelope,
            SearchPlaneQueryIpcResponse,
            StubDispatcher,
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

fn error_response(code: &str, message: impl Into<String>) -> SearchPlaneQueryIpcResponse {
    SearchPlaneQueryIpcResponse::Error(SearchPlaneIpcError {
        code: code.to_string(),
        message: message.into(),
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
    }
}

fn stub_explanation() -> SearchExplanation {
    SearchExplanation {
        planner_trace: vec![PlannerTraceEntry {
            stage: PlannerStage::Merge,
            detail: "rrf fused lexical candidates".to_string(),
        }],
        engines_touched: vec![EngineTouched::Lexical, EngineTouched::Semantic],
        early_stop_reason: None,
        contributions: vec![ExplanationRow {
            signal_name: "bm25".into(),
            signal_value: 0.8,
            weight: 1.0,
            contribution: 0.8,
        }],
        ranker_weights_hash: [7_u8; 32],
        strategy: "rrf".to_string(),
        summary: "fused lexical explanation".to_string(),
    }
}
