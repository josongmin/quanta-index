//! Explain query path: a presence-only explain reports whether a candidate
//!
//! previously returned by a lexical query is in the index, as a typed field
//! decided by exact lookup (QI-BB-022); the scored explain is covered by
//! `e2e_explain_score_trace.rs`.

#![forbid(unsafe_code)]
#![expect(
    clippy::expect_used,
    reason = "integration-test helpers outside `#[test]` fns assert fixture setup with `expect`; the workspace already permits this inside test fns and a helper that cannot set up its fixture has no caller to propagate to"
)]
#![expect(
    clippy::disallowed_methods,
    reason = "test polling paths still use explicit Result fallback checks"
)]
#![expect(
    clippy::wildcard_enum_match_arm,
    reason = "integration response checks intentionally collapse non-target variants"
)]

use std::error::Error;
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::thread;
use std::time::{Duration, Instant};

use quanta_index_contract::lex::LanguageCode;
use quanta_index_contract::{
    BatchIngestMode, ChunkId, ChunkRecord, ExplainCandidateV1, GenerationPin, LexicalCandidate,
    ManifestGeneration, RepoId, RepoRelativePath, RevisionId, SearchCorpusIngestBatch,
    SearchCorpusReplaceScope, SearchPlaneExplainQueryRequest, SearchPlaneIngestIpcRequest,
    SearchPlaneIngestIpcRequestEnvelope, SearchPlaneIngestIpcResponse,
    SearchPlaneIngestIpcResponseEnvelope, SearchPlaneQueryIpcRequest,
    SearchPlaneQueryIpcRequestEnvelope, SearchPlaneQueryIpcResponse,
    SearchPlaneQueryIpcResponseEnvelope, SearchScopeKey, SearchScopeSurface, TextQueryRequest,
    TextQuerySyntax,
};
use quanta_index_ipc::send_request;
use quanta_index_searchd_harness::E2eRuntime;

type TestResult = Result<(), Box<dyn Error>>;
static NEXT_REQUEST_ID: AtomicU64 = AtomicU64::new(1);
const READINESS_TIMEOUT: Duration = Duration::from_secs(5);

/// Harness-owned three-socket scenario fixture (TOPT-03: runtime fixture
/// ownership).
///
/// The daemon's tempdir, three-socket builder, and driver thread all live in
/// [`E2eRuntime`]: boot binds query/control/ingest and waits for all three,
/// and dropping the runtime performs the acknowledged lease-release (signal
/// the driver, join it — which drops the old runtime and releases the
/// state-root lease — before the tempdir is removed). Tests therefore
/// return `Err(..)` directly on failure paths with no manual
/// shutdown/join bookkeeping; teardown is owned by the harness.
struct ScenarioFixture {
    #[expect(
        dead_code,
        reason = "held for drop-order ownership only: the daemon's lifetime is the fixture's, and dropping it performs the acknowledged lease-release"
    )]
    runtime: E2eRuntime,
    query_socket: std::path::PathBuf,
    ingest_socket: std::path::PathBuf,
}

impl ScenarioFixture {
    fn boot() -> Result<Self, Box<dyn Error>> {
        let mut runtime = E2eRuntime::boot()?;
        // Eager start surfaces a boot refusal here and binds all three
        // sockets before any byte is published.
        runtime.start()?;
        let (query_socket, ingest_socket) = {
            let (query, _, ingest) = runtime
                .socket_paths()
                .ok_or_else(|| "fixture: driver started without socket paths".to_string())?;
            (query.to_path_buf(), ingest.to_path_buf())
        };
        Ok(Self {
            runtime,
            query_socket,
            ingest_socket,
        })
    }
}

fn repo() -> RepoId {
    RepoId::new("repo-exp").expect("static fixture ID satisfies canonical policy")
}

fn revision() -> RevisionId {
    RevisionId::new("rev-exp").expect("static fixture ID satisfies canonical policy")
}

fn generation() -> ManifestGeneration {
    ManifestGeneration::new(11)
}

fn chunk_record(id: &str, text: &str) -> Result<ChunkRecord, Box<dyn Error>> {
    Ok(ChunkRecord {
        chunk_id: ChunkId::new(id),
        repo_relative_path: RepoRelativePath::new("src/explain.txt"),
        language: LanguageCode::new("text")
            .map_err(|err| -> Box<dyn Error> { format!("invalid language code: {err}").into() })?,
        start_byte: 0,
        end_byte: u32::try_from(text.len()).map_err(|err| -> Box<dyn Error> {
            format!("chunk text length overflow: {err}").into()
        })?,
        start_line: 0,
        end_line: 0,
        text: text.to_string().into_boxed_str(),
        structural: None,
        parent_chunk_id: None,
        source_repo_id: None,
    })
}

fn send_query_request(
    socket: &Path,
    request: &SearchPlaneQueryIpcRequestEnvelope,
) -> Result<SearchPlaneQueryIpcResponseEnvelope, quanta_index_ipc::IpcError> {
    send_request(socket, request, quanta_index_ipc::ClientIoPolicy::default())
}

fn send_ingest_request(
    socket: &Path,
    request: &SearchPlaneIngestIpcRequestEnvelope,
) -> Result<SearchPlaneIngestIpcResponseEnvelope, quanta_index_ipc::IpcError> {
    send_request(socket, request, quanta_index_ipc::ClientIoPolicy::default())
}

fn wait_until<F>(timeout: Duration, mut cond: F) -> bool
where
    F: FnMut() -> bool,
{
    let start = Instant::now();
    while start.elapsed() < timeout {
        if cond() {
            return true;
        }
        thread::sleep(Duration::from_millis(10));
    }
    false
}

fn lex_query(needle: &str, pin: GenerationPin) -> SearchPlaneQueryIpcRequestEnvelope {
    SearchPlaneQueryIpcRequestEnvelope {
        request_id: 1,
        payload: SearchPlaneQueryIpcRequest::Text(TextQueryRequest {
            syntax: TextQuerySyntax::Sourcegraph,
            query_text: needle.to_string(),
            constraints: quanta_index_contract::QueryConstraintSetV1::unconstrained(),
            generation: Some(pin),
            generation_selector: None,
            top_k: 50,
            cursor: None,
        }),
    }
}

fn explain_request(
    pin: GenerationPin,
    candidate: LexicalCandidate,
) -> SearchPlaneQueryIpcRequestEnvelope {
    SearchPlaneQueryIpcRequestEnvelope {
        request_id: 2,
        payload: SearchPlaneQueryIpcRequest::Explain(SearchPlaneExplainQueryRequest {
            generation: pin,
            candidate: ExplainCandidateV1::Lexical(candidate),
            text_query: None,
            semantic_query_text: None,
        }),
    }
}

fn scope_key(path: &str) -> SearchScopeKey {
    SearchScopeKey {
        doc_surface: SearchScopeSurface::Chunk,
        repo_relative_path: RepoRelativePath::new(path),
    }
}

fn dispatch_ingest(socket: &Path, payload: SearchPlaneIngestIpcRequest) -> TestResult {
    // Like every producer, stamp the canonical batch digest before sending
    // (QI-BB-032); the search plane refuses any other digest.
    let payload = quanta_index_searchd_harness::stamped_ingest_request(payload)?;
    let response = send_ingest_request(
        socket,
        &SearchPlaneIngestIpcRequestEnvelope {
            request_id: NEXT_REQUEST_ID.fetch_add(1, Ordering::Relaxed),
            payload,
        },
    )?;
    match response.payload {
        SearchPlaneIngestIpcResponse::Error(err) => {
            Err(format!("ingest failed code={} message={}", err.code, err.message).into())
        }
        _ => Ok(()),
    }
}

fn publish_chunk(socket: &Path, chunk: ChunkRecord) -> TestResult {
    dispatch_ingest(
        socket,
        SearchPlaneIngestIpcRequest::PublishSearchCorpusBatch(SearchCorpusIngestBatch {
            repo_id: repo(),
            revision_id: revision(),
            generation: generation(),
            base_generation: None,
            manifest_digest: format!("explain-lex-manifest-{}", generation().get()),
            batch_digest: String::new(),
            mode: BatchIngestMode::ReplaceGeneration,
            bundle_payload: None,
            clear_surfaces: Vec::new(),
            replace_scopes: vec![SearchCorpusReplaceScope {
                scope: scope_key(chunk.repo_relative_path.as_str()),
                scope_digest: "explain-scope".to_string(),
                chunks: vec![chunk],
                symbols: Vec::new(),
            }],
            tombstone_scopes: Vec::new(),
            semantic_replace_scopes: Vec::new(),
            semantic_tombstone_scopes: Vec::new(),
            seal: false,
        }),
    )
}

fn seal_lexical(socket: &Path) -> TestResult {
    dispatch_ingest(
        socket,
        SearchPlaneIngestIpcRequest::PublishSearchCorpusBatch(SearchCorpusIngestBatch {
            repo_id: repo(),
            revision_id: revision(),
            generation: generation(),
            base_generation: None,
            manifest_digest: format!("explain-lex-seal-{}", generation().get()),
            batch_digest: String::new(),
            mode: BatchIngestMode::ReplaceGeneration,
            bundle_payload: None,
            clear_surfaces: Vec::new(),
            replace_scopes: Vec::new(),
            tombstone_scopes: Vec::new(),
            semantic_replace_scopes: Vec::new(),
            semantic_tombstone_scopes: Vec::new(),
            seal: true,
        }),
    )
}

#[test]
fn explain_reports_present_candidate() -> TestResult {
    let fixture = ScenarioFixture::boot()?;
    let socket = fixture.query_socket.clone();
    let ingest_socket = fixture.ingest_socket;
    publish_chunk(
        &ingest_socket,
        chunk_record("explain-c1", "quick brown fox jumps")?,
    )?;
    seal_lexical(&ingest_socket)?;

    let pin = GenerationPin::new(repo(), revision(), generation());
    if !wait_until(READINESS_TIMEOUT, || {
        send_query_request(&socket, &lex_query("quick", pin.clone()))
            .map(|r| !matches!(r.payload, SearchPlaneQueryIpcResponse::Error(_)))
            .unwrap_or(false)
    }) {
        return Err("dispatcher never sealed".into());
    }

    let lex_resp = send_query_request(&socket, &lex_query("quick", pin.clone()))?;
    let candidate = match lex_resp.payload {
        SearchPlaneQueryIpcResponse::Text(lex) => lex
            .results
            .into_iter()
            .next()
            .ok_or_else(|| Box::<dyn Error>::from("lexical query returned zero candidates"))?,
        other => {
            return Err(format!("expected Lexical, got {other:?}").into());
        }
    };
    if candidate.candidate_id != "explain-c1" {
        return Err(format!("expected explain-c1, got {}", candidate.candidate_id).into());
    }

    let explain_resp = send_query_request(&socket, &explain_request(pin, candidate))?;
    let (presence, explanation) = match explain_resp.payload {
        SearchPlaneQueryIpcResponse::Explain(exp) => (exp.presence, exp.explanation),
        other => {
            return Err(format!("expected Explain, got {other:?}").into());
        }
    };
    if presence != quanta_index_contract::CandidatePresenceV1::Indexed
        || explanation.strategy != "presence_lookup"
    {
        return Err(format!(
            "expected a typed indexed presence from an exact lookup, got {presence:?} / {}",
            explanation.strategy
        )
        .into());
    }
    if !explanation.summary.contains("present") {
        return Err(format!(
            "expected 'present' in explanation summary, got: {}",
            explanation.summary
        )
        .into());
    }
    if !explanation.summary.contains("explain-c1") {
        return Err(format!(
            "expected candidate id in summary, got: {}",
            explanation.summary
        )
        .into());
    }

    Ok(())
}

#[test]
fn explain_rejects_generation_mismatch() -> TestResult {
    let fixture = ScenarioFixture::boot()?;
    let socket = fixture.query_socket.clone();
    let ingest_socket = fixture.ingest_socket;
    publish_chunk(
        &ingest_socket,
        chunk_record("c-mismatch", "alpha bravo charlie")?,
    )?;
    seal_lexical(&ingest_socket)?;

    let pin = GenerationPin::new(repo(), revision(), generation());
    if !wait_until(READINESS_TIMEOUT, || {
        send_query_request(&socket, &lex_query("alpha", pin.clone()))
            .map(|r| !matches!(r.payload, SearchPlaneQueryIpcResponse::Error(_)))
            .unwrap_or(false)
    }) {
        return Err("dispatcher never sealed".into());
    }

    // Construct a candidate whose manifest_generation differs from the pin.
    let stale_candidate = LexicalCandidate {
        candidate_id: "c-mismatch".to_string(),
        repo_id: repo(),
        revision_id: revision(),
        manifest_generation: ManifestGeneration::new(99),
        repo_relative_path: RepoRelativePath::new(""),
        start_line: 0,
        end_line: 0,
        score: 1.0,
        snippet: "alpha bravo charlie".to_string(),
        snippet_hit_offset: None,
        highlights: Vec::new(),
    };
    let resp = send_query_request(&socket, &explain_request(pin, stale_candidate))?;
    let err = match resp.payload {
        SearchPlaneQueryIpcResponse::Error(e) => e,
        other => {
            return Err(format!("expected Error, got {other:?}").into());
        }
    };
    if err.code.as_wire_str() != "INVALID_REQUEST" {
        return Err(format!("expected INVALID_REQUEST, got {}", err.code).into());
    }

    Ok(())
}
