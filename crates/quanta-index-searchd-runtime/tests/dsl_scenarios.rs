//! Complex DSL scenario pack for the live repo-first search-plane.

#![forbid(unsafe_code)]
#![expect(
    clippy::expect_used,
    reason = "integration-test helpers outside `#[test]` fns assert fixture setup with `expect`; the workspace already permits this inside test fns and a helper that cannot set up its fixture has no caller to propagate to"
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
    BatchIngestMode, ChunkId, ChunkRecord, EarlyStopReason, EngineTouched, GenerationPin,
    HybridQueryRequest, LexicalCandidate, LqVisibility, ManifestGeneration, PlannerStage, RepoId,
    RepoRelativePath, RevisionId, SearchCorpusIngestBatch, SearchCorpusReplaceScope,
    SearchPlaneIngestIpcRequest, SearchPlaneIngestIpcRequestEnvelope, SearchPlaneIngestIpcResponse,
    SearchPlaneIngestIpcResponseEnvelope, SearchPlaneQueryIpcRequest,
    SearchPlaneQueryIpcRequestEnvelope, SearchPlaneQueryIpcResponse,
    SearchPlaneQueryIpcResponseEnvelope, SearchScopeKey, SearchScopeSurface, SemanticQueryRequest,
    TextQueryRequest, TextQuerySyntax,
};
use quanta_index_ipc::send_request;
use quanta_index_searchd_harness::E2eRuntime;
use serde::ser::{Serialize, SerializeStruct, Serializer};
use std::collections::BTreeMap;

use crate::frontdoor_scenarios::DSL_FRONTDOOR_SCENARIOS;

type TestResult = Result<(), Box<dyn Error>>;
static NEXT_REQUEST_ID: AtomicU64 = AtomicU64::new(1);
const READINESS_TIMEOUT: Duration = Duration::from_secs(15);

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

struct RepoMetadataPayload<'a> {
    fork: bool,
    archived: bool,
    visibility: LqVisibility,
    contexts: &'a [&'a str],
}

impl Serialize for RepoMetadataPayload<'_> {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut state = serializer.serialize_struct("RepoMetadataPayload", 4)?;
        state.serialize_field("fork", &self.fork)?;
        state.serialize_field("archived", &self.archived)?;
        state.serialize_field("visibility", &self.visibility)?;
        state.serialize_field("contexts", &self.contexts)?;
        state.end()
    }
}

fn repo() -> RepoId {
    RepoId::new("repo-dsl").expect("static fixture ID satisfies canonical policy")
}

fn revision() -> RevisionId {
    RevisionId::new("rev-dsl").expect("static fixture ID satisfies canonical policy")
}

fn generation() -> ManifestGeneration {
    ManifestGeneration::new(21)
}

fn chunk_record(id: &str, text: &str) -> Result<ChunkRecord, Box<dyn Error>> {
    chunk_record_with_metadata(id, "src/dsl.txt", "text", 0, 0, text)
}

fn chunk_record_with_metadata(
    id: &str,
    repo_relative_path: &str,
    language: &str,
    start_line: u32,
    end_line: u32,
    text: &str,
) -> Result<ChunkRecord, Box<dyn Error>> {
    Ok(ChunkRecord {
        chunk_id: ChunkId::new(id),
        repo_relative_path: RepoRelativePath::new(repo_relative_path),
        language: LanguageCode::new(language)
            .map_err(|err| -> Box<dyn Error> { format!("invalid language code: {err}").into() })?,
        start_byte: 0,
        end_byte: u32::try_from(text.len()).map_err(|err| -> Box<dyn Error> {
            format!("chunk text length overflow: {err}").into()
        })?,
        start_line,
        end_line,
        text: text.to_string().into_boxed_str(),
        structural: None,
        parent_chunk_id: None,
        source_repo_id: None,
    })
}

fn repo_metadata_payload(
    fork: bool,
    archived: bool,
    visibility: LqVisibility,
    contexts: &[&str],
) -> Result<Vec<u8>, Box<dyn Error>> {
    let record = RepoMetadataPayload {
        fork,
        archived,
        visibility,
        contexts,
    };
    let mut buf = Vec::new();
    ciborium::into_writer(&record, &mut buf)
        .map_err(|err| -> Box<dyn Error> { format!("encode repo metadata: {err}").into() })?;
    Ok(buf)
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

fn pin() -> GenerationPin {
    GenerationPin::new(repo(), revision(), generation())
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

fn publish_search_corpus_chunks(
    socket: &Path,
    chunks: Vec<ChunkRecord>,
    bundle_payload: Option<Vec<u8>>,
) -> TestResult {
    let mut chunks_by_path: BTreeMap<String, Vec<ChunkRecord>> = BTreeMap::new();
    for chunk in chunks {
        chunks_by_path
            .entry(chunk.repo_relative_path.as_str().to_string())
            .or_default()
            .push(chunk);
    }
    let replace_scopes = chunks_by_path
        .into_iter()
        .map(|(path, chunks)| SearchCorpusReplaceScope {
            scope: scope_key(&path),
            scope_digest: format!("dsl-lex-scope:{path}"),
            chunks,
            symbols: Vec::new(),
        })
        .collect();
    dispatch_ingest(
        socket,
        SearchPlaneIngestIpcRequest::PublishSearchCorpusBatch(SearchCorpusIngestBatch {
            repo_id: repo(),
            revision_id: revision(),
            generation: generation(),
            base_generation: None,
            manifest_digest: format!("dsl-lex-manifest-{}", generation().get()),
            batch_digest: String::new(),
            mode: BatchIngestMode::ReplaceGeneration,
            bundle_payload,
            clear_surfaces: Vec::new(),
            replace_scopes,
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
            manifest_digest: format!("dsl-lex-seal-{}", generation().get()),
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

fn lexical_request(
    request_id: u64,
    syntax: TextQuerySyntax,
    query_text: &str,
) -> SearchPlaneQueryIpcRequestEnvelope {
    SearchPlaneQueryIpcRequestEnvelope {
        request_id,
        payload: SearchPlaneQueryIpcRequest::Text(TextQueryRequest {
            syntax,
            query_text: query_text.to_string(),
            constraints: quanta_index_contract::QueryConstraintSetV1::unconstrained(),
            generation: Some(pin()),
            generation_selector: None,
            top_k: 50,
            cursor: None,
        }),
    }
}

fn lexical_ids(results: &[LexicalCandidate]) -> Vec<String> {
    results
        .iter()
        .map(|candidate| candidate.candidate_id.clone())
        .collect()
}

fn sort_ids(mut ids: Vec<String>) -> Vec<String> {
    ids.sort();
    ids
}

fn wait_for_non_error(socket: &Path, request: &SearchPlaneQueryIpcRequestEnvelope) -> bool {
    wait_until(READINESS_TIMEOUT, || {
        match send_query_request(socket, request) {
            Ok(response) => !matches!(response.payload, SearchPlaneQueryIpcResponse::Error(_)),
            Err(_) => false,
        }
    })
}

#[test]
fn sourcegraph_repo_path_lang_filters_are_deterministic_across_repeated_runs() -> TestResult {
    let fixture = ScenarioFixture::boot()?;
    let socket = fixture.query_socket.clone();
    let ingest_socket = fixture.ingest_socket.clone();
    publish_search_corpus_chunks(
        &ingest_socket,
        vec![
            chunk_record_with_metadata("alpha", "src/lib.rs", "rust", 10, 14, "needle rust alpha")?,
            chunk_record_with_metadata("beta", "src/main.rs", "rust", 21, 26, "needle rust beta")?,
            chunk_record_with_metadata(
                "gamma",
                "src/lib.py",
                "python",
                30,
                35,
                "needle python gamma",
            )?,
        ],
        Some(repo_metadata_payload(
            false,
            false,
            LqVisibility::Public,
            &["global", "team-search"],
        )?),
    )?;
    seal_lexical(&ingest_socket)?;

    let query_text = "repo:repo-dsl path:src/lib.rs lang:rust fork:no archived:no visibility:public context:global needle";
    let request = lexical_request(2, TextQuerySyntax::Sourcegraph, query_text);
    if !wait_for_non_error(&socket, &request) {
        return Err("sourcegraph metadata query never became ready".into());
    }

    for _ in 0..5_u8 {
        let response = send_query_request(&socket, &request)?;
        let results = match response.payload {
            SearchPlaneQueryIpcResponse::Text(lexical) => lexical.results,
            other @ (SearchPlaneQueryIpcResponse::Symbol(_)
            | SearchPlaneQueryIpcResponse::Semantic(_)
            | SearchPlaneQueryIpcResponse::Hybrid(_)
            | SearchPlaneQueryIpcResponse::HybridSeed(_)
            | SearchPlaneQueryIpcResponse::History(_)
            | SearchPlaneQueryIpcResponse::Structural(_)
            | SearchPlaneQueryIpcResponse::RepoMapQuery(_)
            | SearchPlaneQueryIpcResponse::Explain(_)
            | SearchPlaneQueryIpcResponse::Error(_)
            | quanta_index_contract::SearchPlaneQueryIpcResponse::ClusterMembershipRead(
                _,
            )
            | quanta_index_contract::SearchPlaneQueryIpcResponse::RuntimeMetadata(_)) => {
                return Err(format!("expected Text, got {other:?}").into());
            }
        };
        if results.len() != 1 {
            return Err(format!("expected 1 metadata-filtered hit, got {results:?}").into());
        }
        let candidate = results
            .first()
            .ok_or_else(|| "metadata-filtered result missing first candidate".to_string())?;
        if candidate.candidate_id != "alpha" {
            return Err(format!("expected alpha, got {}", candidate.candidate_id).into());
        }
        if candidate.repo_relative_path.as_str() != "src/lib.rs" {
            return Err(format!(
                "expected src/lib.rs path, got {}",
                candidate.repo_relative_path.as_str()
            )
            .into());
        }
        if candidate.start_line != 10 || candidate.end_line != 14 {
            return Err(format!(
                "expected line span 10..14, got {}..{}",
                candidate.start_line, candidate.end_line
            )
            .into());
        }
    }

    let negative_request = lexical_request(3, TextQuerySyntax::Sourcegraph, "archived:only needle");
    let negative_response = send_query_request(&socket, &negative_request)?;
    let negative_results = match negative_response.payload {
        SearchPlaneQueryIpcResponse::Text(lexical) => lexical.results,
        other => {
            return Err(format!("expected Text for archived negative query, got {other:?}").into());
        }
    };
    if !negative_results.is_empty() {
        return Err(format!(
            "expected archived:only to return 0 hits for non-archived repo, got {:?}",
            lexical_ids(&negative_results)
        )
        .into());
    }

    Ok(())
}

#[test]
fn sourcegraph_boolean_text_query_is_deterministic_across_repeated_runs() -> TestResult {
    let fixture = ScenarioFixture::boot()?;
    let socket = fixture.query_socket.clone();
    let ingest_socket = fixture.ingest_socket.clone();
    publish_search_corpus_chunks(
        &ingest_socket,
        vec![
            chunk_record("alpha", "sphinx alpha needle")?,
            chunk_record("beta", "beta needle")?,
            chunk_record("gamma", "sphinx gamma")?,
            chunk_record("delta", "beta needle forbidden")?,
        ],
        Some(b"manifest".to_vec()),
    )?;
    seal_lexical(&ingest_socket)?;

    let query_text = "(sphinx OR beta) needle NOT forbidden";
    let request = lexical_request(1, TextQuerySyntax::Sourcegraph, query_text);
    if !wait_for_non_error(&socket, &request) {
        return Err("sourcegraph boolean query never became ready".into());
    }

    let mut baseline: Option<Vec<String>> = None;
    for _ in 0..10_u8 {
        let response = send_query_request(&socket, &request)?;
        let ids = match response.payload {
            SearchPlaneQueryIpcResponse::Text(lexical) => lexical_ids(&lexical.results),
            other @ (SearchPlaneQueryIpcResponse::Symbol(_)
            | SearchPlaneQueryIpcResponse::Semantic(_)
            | SearchPlaneQueryIpcResponse::Hybrid(_)
            | SearchPlaneQueryIpcResponse::HybridSeed(_)
            | SearchPlaneQueryIpcResponse::History(_)
            | SearchPlaneQueryIpcResponse::Structural(_)
            | SearchPlaneQueryIpcResponse::RepoMapQuery(_)
            | SearchPlaneQueryIpcResponse::Explain(_)
            | SearchPlaneQueryIpcResponse::Error(_)
            | quanta_index_contract::SearchPlaneQueryIpcResponse::ClusterMembershipRead(
                _,
            )
            | quanta_index_contract::SearchPlaneQueryIpcResponse::RuntimeMetadata(_)) => {
                return Err(format!("expected Lexical, got {other:?}").into());
            }
        };
        if let Some(expected) = baseline.as_ref() {
            if &ids != expected {
                return Err(format!(
                    "lexical ordering drifted across repeated runs: expected {expected:?}, got {ids:?}"
                )
                .into());
            }
        } else {
            baseline = Some(ids);
        }
    }

    let observed = baseline.ok_or_else(|| "missing baseline lexical ids".to_string())?;
    if sort_ids(observed.clone()) != ["alpha".to_string(), "beta".to_string()] {
        return Err(format!("unexpected sourcegraph lexical ids: {observed:?}").into());
    }

    Ok(())
}

#[test]
fn sourcegraph_repo_has_file_predicate_executes_live() -> TestResult {
    let fixture = ScenarioFixture::boot()?;
    let socket = fixture.query_socket.clone();
    let ingest_socket = fixture.ingest_socket.clone();
    publish_search_corpus_chunks(
        &ingest_socket,
        vec![
            chunk_record_with_metadata("alpha", "src/lib.rs", "rust", 1, 2, "needle alpha")?,
            chunk_record_with_metadata("beta", "src/main.rs", "rust", 1, 2, "needle beta")?,
            chunk_record_with_metadata("gamma", "docs/readme.md", "rust", 1, 2, "other text")?,
        ],
        Some(b"manifest".to_vec()),
    )?;
    seal_lexical(&ingest_socket)?;

    for (idx, &scenario) in DSL_FRONTDOOR_SCENARIOS.iter().enumerate() {
        let request = lexical_request(
            u64::try_from(13 + idx).map_err(|err| -> Box<dyn Error> {
                format!("repo.has.file request id overflow: {err}").into()
            })?,
            scenario.syntax,
            scenario.query_text,
        );
        if !wait_for_non_error(&socket, &request) {
            return Err(format!("{} never became ready", scenario.name).into());
        }
        let ids = match send_query_request(&socket, &request)?.payload {
            SearchPlaneQueryIpcResponse::Text(lexical) => lexical_ids(&lexical.results),
            other @ (SearchPlaneQueryIpcResponse::Symbol(_)
            | SearchPlaneQueryIpcResponse::Semantic(_)
            | SearchPlaneQueryIpcResponse::Hybrid(_)
            | SearchPlaneQueryIpcResponse::HybridSeed(_)
            | SearchPlaneQueryIpcResponse::History(_)
            | SearchPlaneQueryIpcResponse::Structural(_)
            | SearchPlaneQueryIpcResponse::RepoMapQuery(_)
            | SearchPlaneQueryIpcResponse::Explain(_)
            | SearchPlaneQueryIpcResponse::Error(_)
            | quanta_index_contract::SearchPlaneQueryIpcResponse::ClusterMembershipRead(
                _,
            )
            | quanta_index_contract::SearchPlaneQueryIpcResponse::RuntimeMetadata(_)) => {
                return Err(format!("{} expected Lexical, got {other:?}", scenario.name).into());
            }
        };
        let expected = scenario
            .expected_candidate_ids
            .iter()
            .map(|id| (*id).to_string())
            .collect::<Vec<_>>();
        if sort_ids(ids.clone()) != expected {
            return Err(format!(
                "{} ids diverged: expected {:?}, got {:?}",
                scenario.name, expected, ids
            )
            .into());
        }
    }

    Ok(())
}

#[test]
fn sourcegraph_phrase_and_regex_patterns_execute_live() -> TestResult {
    let fixture = ScenarioFixture::boot()?;
    let socket = fixture.query_socket.clone();
    let ingest_socket = fixture.ingest_socket.clone();
    publish_search_corpus_chunks(
        &ingest_socket,
        vec![
            chunk_record("alpha", "sphinx of quartz")?,
            chunk_record("beta", "riddle42")?,
            chunk_record("gamma", "sphinx of clay")?,
        ],
        None,
    )?;
    seal_lexical(&ingest_socket)?;

    let request = lexical_request(
        11,
        TextQuerySyntax::Sourcegraph,
        "\"sphinx of quartz\" OR /riddle[0-9]+/",
    );
    if !wait_until(READINESS_TIMEOUT, || {
        match send_query_request(&socket, &request) {
            Ok(response) => match response.payload {
                SearchPlaneQueryIpcResponse::Text(_)
                | SearchPlaneQueryIpcResponse::Symbol(_)
                | SearchPlaneQueryIpcResponse::Semantic(_)
                | SearchPlaneQueryIpcResponse::Hybrid(_)
                | SearchPlaneQueryIpcResponse::HybridSeed(_)
                | SearchPlaneQueryIpcResponse::History(_)
                | SearchPlaneQueryIpcResponse::Structural(_)
                | SearchPlaneQueryIpcResponse::RepoMapQuery(_)
                | SearchPlaneQueryIpcResponse::Explain(_)
                | quanta_index_contract::SearchPlaneQueryIpcResponse::ClusterMembershipRead(_)
                | quanta_index_contract::SearchPlaneQueryIpcResponse::RuntimeMetadata(_) => true,
                SearchPlaneQueryIpcResponse::Error(err) => err.code.as_wire_str() != "NOT_READY",
            },
            Err(_) => false,
        }
    }) {
        return Err("sourcegraph phrase/regex query never progressed past NOT_READY".into());
    }

    let response = send_query_request(&socket, &request)?;
    let ids = match response.payload {
        SearchPlaneQueryIpcResponse::Text(lexical) => lexical_ids(&lexical.results),
        other @ (SearchPlaneQueryIpcResponse::Symbol(_)
        | SearchPlaneQueryIpcResponse::Semantic(_)
        | SearchPlaneQueryIpcResponse::Hybrid(_)
        | SearchPlaneQueryIpcResponse::HybridSeed(_)
        | SearchPlaneQueryIpcResponse::History(_)
        | SearchPlaneQueryIpcResponse::Structural(_)
        | SearchPlaneQueryIpcResponse::RepoMapQuery(_)
        | SearchPlaneQueryIpcResponse::Explain(_)
        | SearchPlaneQueryIpcResponse::Error(_)
        | quanta_index_contract::SearchPlaneQueryIpcResponse::ClusterMembershipRead(_)
        | quanta_index_contract::SearchPlaneQueryIpcResponse::RuntimeMetadata(_)) => {
            return Err(format!("expected Lexical, got {other:?}").into());
        }
    };
    if sort_ids(ids.clone()) != ["alpha".to_string(), "beta".to_string()] {
        return Err(format!("unexpected Sourcegraph phrase/regex ids: {ids:?}").into());
    }

    let regexp_option_request = lexical_request(
        12,
        TextQuerySyntax::Sourcegraph,
        "patterntype:regexp riddle[0-9]+",
    );
    if !wait_for_non_error(&socket, &regexp_option_request) {
        return Err("sourcegraph patterntype:regexp query never became ready".into());
    }
    let regexp_option_ids = match send_query_request(&socket, &regexp_option_request)?.payload {
        SearchPlaneQueryIpcResponse::Text(lexical) => lexical_ids(&lexical.results),
        other @ (SearchPlaneQueryIpcResponse::Symbol(_)
        | SearchPlaneQueryIpcResponse::Semantic(_)
        | SearchPlaneQueryIpcResponse::Hybrid(_)
        | SearchPlaneQueryIpcResponse::HybridSeed(_)
        | SearchPlaneQueryIpcResponse::History(_)
        | SearchPlaneQueryIpcResponse::Structural(_)
        | SearchPlaneQueryIpcResponse::RepoMapQuery(_)
        | SearchPlaneQueryIpcResponse::Explain(_)
        | SearchPlaneQueryIpcResponse::Error(_)
        | quanta_index_contract::SearchPlaneQueryIpcResponse::ClusterMembershipRead(_)
        | quanta_index_contract::SearchPlaneQueryIpcResponse::RuntimeMetadata(_)) => {
            return Err(format!("expected Lexical for patterntype:regexp, got {other:?}").into());
        }
    };
    if regexp_option_ids != ["beta".to_string()] {
        return Err(format!(
            "unexpected Sourcegraph patterntype:regexp ids: {regexp_option_ids:?}"
        )
        .into());
    }

    Ok(())
}

#[test]
fn lq_phrase_and_regex_patterns_execute_live() -> TestResult {
    let fixture = ScenarioFixture::boot()?;
    let socket = fixture.query_socket.clone();
    let ingest_socket = fixture.ingest_socket.clone();
    publish_search_corpus_chunks(
        &ingest_socket,
        vec![
            chunk_record("alpha", "sphinx of quartz")?,
            chunk_record("beta", "riddle42")?,
            chunk_record("gamma", "sphinx of clay")?,
        ],
        None,
    )?;
    seal_lexical(&ingest_socket)?;

    let phrase_request = lexical_request(
        2,
        TextQuerySyntax::Native,
        "\"sphinx of quartz\" OR riddle42",
    );
    if !wait_for_non_error(&socket, &phrase_request) {
        return Err("LQ phrase query never became ready".into());
    }

    let response = send_query_request(&socket, &phrase_request)?;
    let ids = match response.payload {
        SearchPlaneQueryIpcResponse::Text(lexical) => lexical_ids(&lexical.results),
        other @ (SearchPlaneQueryIpcResponse::Symbol(_)
        | SearchPlaneQueryIpcResponse::Semantic(_)
        | SearchPlaneQueryIpcResponse::Hybrid(_)
        | SearchPlaneQueryIpcResponse::HybridSeed(_)
        | SearchPlaneQueryIpcResponse::History(_)
        | SearchPlaneQueryIpcResponse::Structural(_)
        | SearchPlaneQueryIpcResponse::RepoMapQuery(_)
        | SearchPlaneQueryIpcResponse::Explain(_)
        | SearchPlaneQueryIpcResponse::Error(_)
        | quanta_index_contract::SearchPlaneQueryIpcResponse::ClusterMembershipRead(_)
        | quanta_index_contract::SearchPlaneQueryIpcResponse::RuntimeMetadata(_)) => {
            return Err(format!("expected Lexical, got {other:?}").into());
        }
    };
    if sort_ids(ids.clone()) != ["alpha".to_string(), "beta".to_string()] {
        return Err(format!("unexpected LQ phrase ids: {ids:?}").into());
    }

    let regex_request = lexical_request(3, TextQuerySyntax::Native, "/riddle[0-9]+/");
    if !wait_for_non_error(&socket, &regex_request) {
        return Err("LQ regex query never became ready".into());
    }
    let regex_ids = match send_query_request(&socket, &regex_request)?.payload {
        SearchPlaneQueryIpcResponse::Text(lexical) => lexical_ids(&lexical.results),
        other @ (SearchPlaneQueryIpcResponse::Symbol(_)
        | SearchPlaneQueryIpcResponse::Semantic(_)
        | SearchPlaneQueryIpcResponse::Hybrid(_)
        | SearchPlaneQueryIpcResponse::HybridSeed(_)
        | SearchPlaneQueryIpcResponse::History(_)
        | SearchPlaneQueryIpcResponse::Structural(_)
        | SearchPlaneQueryIpcResponse::RepoMapQuery(_)
        | SearchPlaneQueryIpcResponse::Explain(_)
        | SearchPlaneQueryIpcResponse::Error(_)
        | quanta_index_contract::SearchPlaneQueryIpcResponse::ClusterMembershipRead(_)
        | quanta_index_contract::SearchPlaneQueryIpcResponse::RuntimeMetadata(_)) => {
            return Err(format!("expected live regex result, got {other:?}").into());
        }
    };
    if regex_ids != ["beta".to_string()] {
        return Err(format!("unexpected LQ regex ids: {regex_ids:?}").into());
    }

    Ok(())
}

#[test]
fn semantic_scoped_query_with_complex_scope_excludes_outsiders_and_explains_scope() -> TestResult {
    let fixture = ScenarioFixture::boot()?;
    let socket = fixture.query_socket.clone();
    let ingest_socket = fixture.ingest_socket.clone();
    publish_search_corpus_chunks(
        &ingest_socket,
        vec![
            chunk_record("alpha", "scope alpha keep")?,
            chunk_record("beta", "scope beta keep")?,
            chunk_record("gamma", "scope alpha outsider")?,
            chunk_record("omega", "global outsider")?,
        ],
        None,
    )?;
    seal_lexical(&ingest_socket)?;

    let request = SearchPlaneQueryIpcRequestEnvelope {
        request_id: 3,
        payload: SearchPlaneQueryIpcRequest::Semantic(SemanticQueryRequest {
            query_text: "scope".to_string(),
            constraints: quanta_index_contract::QueryConstraintSetV1::unconstrained(),
            generation: Some(pin()),
            generation_selector: None,
            lexical_scope: Some(TextQueryRequest {
                syntax: TextQuerySyntax::Sourcegraph,
                query_text: "(alpha OR beta) scope NOT outsider".to_string(),
                constraints: quanta_index_contract::QueryConstraintSetV1::unconstrained(),
                generation: Some(pin()),
                generation_selector: None,
                top_k: 2,
                cursor: None,
            }),
            top_k: 2,
        }),
    };
    if !wait_for_non_error(&socket, &request) {
        return Err("semantic scoped complex query never became ready".into());
    }

    let response = send_query_request(&socket, &request)?;
    let (ids, explanation) = match response.payload {
        SearchPlaneQueryIpcResponse::Semantic(semantic) => {
            (lexical_ids(&semantic.results), semantic.explanation)
        }
        other @ (SearchPlaneQueryIpcResponse::Text(_)
        | SearchPlaneQueryIpcResponse::Symbol(_)
        | SearchPlaneQueryIpcResponse::Hybrid(_)
        | SearchPlaneQueryIpcResponse::HybridSeed(_)
        | SearchPlaneQueryIpcResponse::History(_)
        | SearchPlaneQueryIpcResponse::Structural(_)
        | SearchPlaneQueryIpcResponse::RepoMapQuery(_)
        | SearchPlaneQueryIpcResponse::Explain(_)
        | SearchPlaneQueryIpcResponse::Error(_)
        | quanta_index_contract::SearchPlaneQueryIpcResponse::ClusterMembershipRead(_)
        | quanta_index_contract::SearchPlaneQueryIpcResponse::RuntimeMetadata(_)) => {
            return Err(format!("expected Semantic, got {other:?}").into());
        }
    };
    if sort_ids(ids.clone()) != ["alpha".to_string(), "beta".to_string()] {
        return Err(format!("unexpected semantic scoped ids: {ids:?}").into());
    }
    if explanation.strategy != "semantic_scoped" {
        return Err(format!(
            "unexpected semantic explanation strategy: {}",
            explanation.strategy
        )
        .into());
    }
    if explanation.engines_touched != vec![EngineTouched::Lexical, EngineTouched::Semantic] {
        return Err(format!(
            "unexpected semantic engines_touched: {:?}",
            explanation.engines_touched
        )
        .into());
    }
    if !explanation.summary.contains("text scope of 2") {
        return Err(format!(
            "semantic explanation summary missing scope accounting: {}",
            explanation.summary
        )
        .into());
    }
    let has_scope_plan = explanation
        .planner_trace
        .iter()
        .any(|entry| entry.stage == PlannerStage::Plan && entry.detail == "semantic.scope=true");
    let has_scope_exec = explanation.planner_trace.iter().any(|entry| {
        entry.stage == PlannerStage::ExecFanout
            && entry.detail == "semantic.scope.text_candidates=2"
    });
    // The dense lane names its sealed index contract (QI-BB-027): this
    // fixture is below the index floor, so the lane is exact and sealed.
    let has_dense_lane = explanation.planner_trace.iter().any(|entry| {
        entry.stage == PlannerStage::Plan
            && entry.detail == "dense.index=exact; dense.attestation=sealed"
    });
    if !has_scope_plan || !has_scope_exec || !has_dense_lane {
        return Err(format!(
            "semantic planner trace missing scoped or dense-lane details: {:?}",
            explanation.planner_trace
        )
        .into());
    }

    Ok(())
}

#[test]
fn hybrid_query_reports_complex_scope_explanation_accounting() -> TestResult {
    let fixture = ScenarioFixture::boot()?;
    let socket = fixture.query_socket.clone();
    let ingest_socket = fixture.ingest_socket.clone();
    publish_search_corpus_chunks(
        &ingest_socket,
        vec![
            chunk_record("alpha", "scope alpha keep")?,
            chunk_record("beta", "scope beta keep")?,
            chunk_record("gamma", "scope alpha outsider")?,
            chunk_record("omega", "global outsider")?,
        ],
        None,
    )?;
    seal_lexical(&ingest_socket)?;

    let request = SearchPlaneQueryIpcRequestEnvelope {
        request_id: 4,
        payload: SearchPlaneQueryIpcRequest::Hybrid(HybridQueryRequest {
            text_query: TextQueryRequest {
                syntax: TextQuerySyntax::Sourcegraph,
                query_text: "(alpha OR beta) scope NOT outsider".to_string(),
                constraints: quanta_index_contract::QueryConstraintSetV1::unconstrained(),
                generation: Some(pin()),
                generation_selector: None,
                top_k: 50,
                cursor: None,
            },
            semantic_query_text: "scope".to_string(),
            generation: Some(pin()),
            generation_selector: None,
            top_k: 2,
        }),
    };
    if !wait_for_non_error(&socket, &request) {
        return Err("hybrid complex query never became ready".into());
    }

    let response = send_query_request(&socket, &request)?;
    let (ids, explanation) = match response.payload {
        SearchPlaneQueryIpcResponse::Hybrid(hybrid) => (
            hybrid
                .results
                .iter()
                .map(|row| row.candidate.candidate_id.clone())
                .collect::<Vec<_>>(),
            hybrid.explanation,
        ),
        other @ (SearchPlaneQueryIpcResponse::Text(_)
        | SearchPlaneQueryIpcResponse::Symbol(_)
        | SearchPlaneQueryIpcResponse::Semantic(_)
        | SearchPlaneQueryIpcResponse::HybridSeed(_)
        | SearchPlaneQueryIpcResponse::History(_)
        | SearchPlaneQueryIpcResponse::Structural(_)
        | SearchPlaneQueryIpcResponse::RepoMapQuery(_)
        | SearchPlaneQueryIpcResponse::Explain(_)
        | SearchPlaneQueryIpcResponse::Error(_)
        | quanta_index_contract::SearchPlaneQueryIpcResponse::ClusterMembershipRead(_)
        | quanta_index_contract::SearchPlaneQueryIpcResponse::RuntimeMetadata(_)) => {
            return Err(format!("expected Hybrid, got {other:?}").into());
        }
    };
    if sort_ids(ids.clone()) != ["alpha".to_string(), "beta".to_string()] {
        return Err(format!("unexpected hybrid ids: {ids:?}").into());
    }
    if explanation.strategy != "rrf" {
        return Err(format!(
            "unexpected hybrid explanation strategy: {}",
            explanation.strategy
        )
        .into());
    }
    if explanation.engines_touched != vec![EngineTouched::Lexical, EngineTouched::Semantic] {
        return Err(format!(
            "unexpected hybrid engines_touched: {:?}",
            explanation.engines_touched
        )
        .into());
    }
    let has_plan = explanation.planner_trace.iter().any(|entry| {
        entry.stage == PlannerStage::Plan && entry.detail == "hybrid.internal_top_k=100"
    });
    let has_exec = explanation.planner_trace.iter().any(|entry| {
        entry.stage == PlannerStage::ExecFanout
            && entry.detail
                == "hybrid.lanes=independent; lexical_hits=2; semantic_hits=4; fused_universe=4"
    });
    let has_merge = explanation.planner_trace.iter().any(|entry| {
        entry.stage == PlannerStage::Merge && entry.detail == "hybrid.fused_results=2"
    });
    let has_dense_lane = explanation.planner_trace.iter().any(|entry| {
        entry.stage == PlannerStage::Plan
            && entry.detail == "dense.index=exact; dense.attestation=sealed"
    });
    if !has_plan || !has_exec || !has_merge || !has_dense_lane {
        return Err(format!(
            "hybrid planner trace missing accounting or dense-lane details: {:?}",
            explanation.planner_trace
        )
        .into());
    }
    if explanation.summary != "hybrid fused 2 lexical and 4 semantic candidates into 2 results" {
        return Err(format!(
            "unexpected hybrid explanation summary: {}",
            explanation.summary
        )
        .into());
    }

    Ok(())
}

#[test]
fn hybrid_query_surfaces_truthful_count_reached_early_stop() -> TestResult {
    let fixture = ScenarioFixture::boot()?;
    let socket = fixture.query_socket.clone();
    let ingest_socket = fixture.ingest_socket.clone();
    publish_search_corpus_chunks(
        &ingest_socket,
        vec![
            chunk_record("alpha", "scope alpha keep")?,
            chunk_record("beta", "scope beta keep")?,
            chunk_record("gamma", "scope gamma keep")?,
        ],
        None,
    )?;
    seal_lexical(&ingest_socket)?;

    let request = SearchPlaneQueryIpcRequestEnvelope {
        request_id: 5,
        payload: SearchPlaneQueryIpcRequest::Hybrid(HybridQueryRequest {
            text_query: TextQueryRequest {
                syntax: TextQuerySyntax::Sourcegraph,
                query_text: "scope".to_string(),
                constraints: quanta_index_contract::QueryConstraintSetV1::unconstrained(),
                generation: Some(pin()),
                generation_selector: None,
                top_k: 50,
                cursor: None,
            },
            semantic_query_text: "scope".to_string(),
            generation: Some(pin()),
            generation_selector: None,
            top_k: 2,
        }),
    };
    if !wait_for_non_error(&socket, &request) {
        return Err("hybrid count-reached query never became ready".into());
    }

    let response = send_query_request(&socket, &request)?;
    let explanation = match response.payload {
        SearchPlaneQueryIpcResponse::Hybrid(hybrid) => {
            if hybrid.results.len() != 2 {
                return Err(format!(
                    "expected exactly 2 fused results after top_k cap, got {}",
                    hybrid.results.len()
                )
                .into());
            }
            hybrid.explanation
        }
        other => {
            return Err(format!("expected Hybrid, got {other:?}").into());
        }
    };
    if explanation.early_stop_reason != Some(EarlyStopReason::CountReached) {
        return Err(format!(
            "expected count_reached early_stop_reason, got {:?}",
            explanation.early_stop_reason
        )
        .into());
    }

    Ok(())
}
