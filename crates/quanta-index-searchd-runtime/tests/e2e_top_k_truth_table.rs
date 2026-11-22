//! QI-BB-025 — one `top_k` contract across every query route, at the daemon
//! front door.
//!
//! The public range is `1..=10_000`. Every route must refuse `0`, `10_001`
//! and `u32::MAX`, and must accept `1`, `9_999` and `10_000` — the last of
//! which the continuation probe used to refuse while the domain policies
//! accepted it.
//!
//! The refusal is one policy applied at three gates that share
//! `validate_public_top_k` and the `QUERY_TOP_K_OUT_OF_RANGE` code: the SDK
//! builder refuses before any round trip (pinned in the SDK crate against a
//! stub transport), the wire codec refuses on encode (pinned here: a typed
//! request does not encode), and the dispatcher refuses raw bytes carrying
//! the value with a typed response after decode and dispatch (also pinned
//! here and in the search-plane crate for in-process callers).
//!
//! All eight routes are driven through the same raw IPC probe against a
//! pinned Legacy daemon.
//!
//! Acceptance is stronger than "not refused for `top_k`": the fixture seeds
//! every authority, so each route must *serve* an in-range request with no
//! typed error of any kind, honor `returned <= top_k`, and — where the route
//! answers with a wire window — keep `returned` / `candidate_count` /
//! `has_more` mutually consistent as observed on the wire. A route that
//! quietly turns an accepted value into some other refusal fails the table.

#![forbid(unsafe_code)]

use std::{
    collections::BTreeSet,
    error::Error,
    path::PathBuf,
    time::{Duration, Instant},
};

use quanta_index_contract::{
    BatchIngestMode, CandidateCountV1, ChunkId, ChunkRecord, ContinuationTokenV2, GenerationPin,
    HistoryOrderV1, HistoryQueryRequest, HybridQueryRequest, HybridSeedQueryRequest,
    ManifestGeneration, PUBLIC_TOP_K_MAX, QueryConstraintSetV1, QueryResultWindowV2, RepoId,
    RepoRelativePath, RevisionId, RuntimeMetadataQueryRequest,
    SearchPlaneIngestIpcRequest, SearchPlaneIngestIpcRequestEnvelope, SearchPlaneIngestIpcResponse,
    SearchPlaneIngestIpcResponseEnvelope, SearchPlaneQueryIpcRequest,
    SearchPlaneQueryIpcRequestEnvelope, SearchPlaneQueryIpcResponse,
    SearchPlaneQueryIpcResponseEnvelope, SearchScopeKey, SearchScopeSurface, SemanticQueryRequest,
    SourceFileKey, SourcePublicationEvent, StructuralIngestBatch, StructuralQueryRequest,
    StructuralReplaceScope, StructuralTreeRecord, SymbolCoverage, SymbolId, SymbolQueryRequest,
    TOP_K_OUT_OF_RANGE_CODE, TextQueryRequest, TextQuerySyntax,
    lex::{
        DirtyRecord, LanguageCode, SymbolKindCode, SymbolKindFamily, SymbolRecord,
        SymbolRelationship, SymbolSpan,
    },
};
use quanta_index_ipc::{ClientIoPolicy, encode_request, send_request};
use quanta_index_sdk::{
    ConnectOptions, DirtyBatch, PublishedBatchEvidence, QuantaIndex, SdkError, SearchCorpusBatch,
};
use quanta_index_searchd_harness::{
    fixture_source_scope_v1,
    history_fixture::{HISTORY_LEDGER, HistoryFixtureSpec},
    legacy_reference::{
        LegacySearchdBinary, SearchdBinaryProcess, daemon_socket_paths,
        legacy_source_replacement_from_chunks, publish_legacy_source_batch, request_when_ready,
    },
    private_tempdir,
    scale_runtime_policy::DEFAULT_HISTORY_MAX_GENERATIONS,
    semantic_source_scopes_for_chunk_records, stamped_ingest_request,
    structural_function_tree_record,
};

type TestResult = Result<(), Box<dyn Error>>;

const REFUSED: [u32; 3] = [0, PUBLIC_TOP_K_MAX + 1, u32::MAX];
const ACCEPTED: [u32; 3] = [1, PUBLIC_TOP_K_MAX - 1, PUBLIC_TOP_K_MAX];

/// A query route as the truth table sees it: a name and a payload builder
/// over the harness's sealed pin.
struct Route {
    name: &'static str,
    build: fn(Option<GenerationPin>, u32) -> SearchPlaneQueryIpcRequest,
}

/// Query text per authority.
///
/// Every route needs a query its lowering accepts, otherwise the route refuses
/// for the query before it ever reaches the `top_k` gate and the table would
/// be measuring the wrong thing.
const LEXICAL_QUERY: &str = "needle";
const SYMBOL_QUERY: &str = "symbol.has.name(needle)";
/// The history query names a whole token of the fixture's commit message.
///
/// The message is `fix: sample history alpha_content_needle`; history
/// keywords match whole folded tokens (QI-BB-011/023: one tokenizer for
/// every text route, `_` is a token character), not substrings.
const HISTORY_QUERY: &str = "type:commit alpha_content_needle";
const RUNTIME_QUERY: &str = "dirty:only needle";
const STRUCTURAL_QUERY: &str = "match { function_item { { identifier :[name] } } }";

fn text_request(
    syntax: TextQuerySyntax,
    query_text: &str,
    pin: Option<GenerationPin>,
    top_k: u32,
) -> TextQueryRequest {
    TextQueryRequest {
        syntax,
        query_text: query_text.to_string(),
        constraints: QueryConstraintSetV1::unconstrained(),
        generation: pin,
        generation_selector: None,
        top_k,
        cursor: None,
    }
}

fn native_request(pin: Option<GenerationPin>, top_k: u32) -> TextQueryRequest {
    text_request(TextQuerySyntax::Native, LEXICAL_QUERY, pin, top_k)
}

fn lexical(pin: Option<GenerationPin>, top_k: u32) -> SearchPlaneQueryIpcRequest {
    SearchPlaneQueryIpcRequest::Text(native_request(pin, top_k))
}

fn symbol(pin: Option<GenerationPin>, top_k: u32) -> SearchPlaneQueryIpcRequest {
    SearchPlaneQueryIpcRequest::Symbol(SymbolQueryRequest {
        syntax: TextQuerySyntax::Native,
        query_text: SYMBOL_QUERY.to_string(),
        constraints: QueryConstraintSetV1::unconstrained(),
        generation: pin,
        generation_selector: None,
        top_k,
        cursor: None,
    })
}

fn semantic(pin: Option<GenerationPin>, top_k: u32) -> SearchPlaneQueryIpcRequest {
    SearchPlaneQueryIpcRequest::Semantic(SemanticQueryRequest {
        query_text: LEXICAL_QUERY.to_string(),
        constraints: QueryConstraintSetV1::unconstrained(),
        generation: pin,
        generation_selector: None,
        lexical_scope: None,
        top_k,
    })
}

fn hybrid(pin: Option<GenerationPin>, top_k: u32) -> SearchPlaneQueryIpcRequest {
    SearchPlaneQueryIpcRequest::Hybrid(HybridQueryRequest {
        text_query: native_request(pin.clone(), top_k),
        semantic_query_text: LEXICAL_QUERY.to_string(),
        generation: pin,
        generation_selector: None,
        top_k,
    })
}

fn hybrid_seed(pin: Option<GenerationPin>, top_k: u32) -> SearchPlaneQueryIpcRequest {
    SearchPlaneQueryIpcRequest::HybridSeed(HybridSeedQueryRequest {
        text_query: native_request(pin.clone(), top_k),
        semantic_query_text: LEXICAL_QUERY.to_string(),
        generation: pin,
        generation_selector: None,
        dense_corpora: Vec::new(),
        top_k,
    })
}

fn history(pin: Option<GenerationPin>, top_k: u32) -> SearchPlaneQueryIpcRequest {
    SearchPlaneQueryIpcRequest::History(HistoryQueryRequest {
        text_query: text_request(TextQuerySyntax::Sourcegraph, HISTORY_QUERY, pin, top_k),
        order: HistoryOrderV1::Recency,
        cursor: None,
    })
}

fn runtime_metadata(pin: Option<GenerationPin>, top_k: u32) -> SearchPlaneQueryIpcRequest {
    SearchPlaneQueryIpcRequest::RuntimeMetadata(RuntimeMetadataQueryRequest {
        text_query: text_request(TextQuerySyntax::Sourcegraph, RUNTIME_QUERY, pin, top_k),
        cursor: None,
    })
}

fn structural(pin: Option<GenerationPin>, top_k: u32) -> SearchPlaneQueryIpcRequest {
    SearchPlaneQueryIpcRequest::Structural(StructuralQueryRequest {
        text_query: text_request(TextQuerySyntax::Native, STRUCTURAL_QUERY, pin, top_k),
        cursor: None,
    })
}

/// The eight `top_k`-bearing routes named by the finding's completion criteria.
///
/// Every one of them serves on the full fixture, so an in-range `top_k` has
/// exactly one acceptable outcome: no typed error at all.
const ROUTES: [Route; 8] = [
    Route {
        name: "lexical",
        build: lexical,
    },
    Route {
        name: "symbol",
        build: symbol,
    },
    Route {
        name: "semantic",
        build: semantic,
    },
    Route {
        name: "hybrid",
        build: hybrid,
    },
    Route {
        name: "hybrid_seed",
        build: hybrid_seed,
    },
    Route {
        name: "history",
        build: history,
    },
    Route {
        name: "runtime_metadata",
        build: runtime_metadata,
    },
    Route {
        name: "structural",
        build: structural,
    },
];

struct TopKRuntime {
    daemon: SearchdBinaryProcess,
    binary: LegacySearchdBinary,
    client: QuantaIndex,
    _root: tempfile::TempDir,
    query_socket: PathBuf,
    ingest_socket: PathBuf,
    io: ClientIoPolicy,
    pin: GenerationPin,
}

impl TopKRuntime {
    fn start(timeout: Duration, history_bytes: u64) -> Result<Self, Box<dyn Error>> {
        let root = private_tempdir()?;
        let binary = LegacySearchdBinary::from_environment()?;
        let mut command = binary.benchmark_command(root.path(), DEFAULT_HISTORY_MAX_GENERATIONS)?;
        let _configured = command.env(
            "QUANTA_INDEX_SEARCH_CORPUS_HISTORY_MAX_BYTES",
            history_bytes.to_string(),
        );
        let daemon = SearchdBinaryProcess::start_with_pinned_command(root.path(), command)?;
        let client = QuantaIndex::connect_legacy(
            ConnectOptions::from_state_root(root.path()).with_request_io_timeout(timeout),
        )?;
        let [query_socket, _, ingest_socket] = daemon_socket_paths(root.path());
        Ok(Self {
            daemon,
            binary,
            client,
            _root: root,
            query_socket,
            ingest_socket,
            io: ClientIoPolicy::try_new(timeout)?,
            pin: GenerationPin::new(
                RepoId::new("repo-e2e")?,
                RevisionId::new("rev-e2e")?,
                ManifestGeneration::new(1),
            ),
        })
    }

    fn stop(self) -> TestResult {
        self.daemon.stop()?;
        self.binary.verify()
    }

    fn query(
        &self,
        payload: SearchPlaneQueryIpcRequest,
    ) -> Result<SearchPlaneQueryIpcResponse, SdkError> {
        let request = SearchPlaneQueryIpcRequestEnvelope {
            request_id: 1,
            diagnostic_attempt: quanta_index_contract::DiagnosticAttemptFieldV1::Absent,
            payload,
        };
        request_when_ready(|| {
            let response: SearchPlaneQueryIpcResponseEnvelope =
                send_request(&self.query_socket, &request, self.io).map_err(SdkError::Transport)?;
            match response.payload {
                SearchPlaneQueryIpcResponse::Error(error) => Err(SdkError::Remote {
                    code: error.code,
                    message: error.message,
                    repair: error.repair,
                }),
                payload => Ok(payload),
            }
        })
    }
}

fn fixture_chunk(
    id: ChunkId,
    path: &str,
    body: &str,
    start_line: u32,
    end_line: u32,
) -> Result<ChunkRecord, Box<dyn Error>> {
    Ok(ChunkRecord {
        chunk_id: id,
        repo_relative_path: RepoRelativePath::new(path),
        language: LanguageCode::new("rust")?,
        start_byte: 0,
        end_byte: u32::try_from(body.len())?,
        start_line,
        end_line,
        text: body.into(),
        structural: None,
        parent_chunk_id: None,
        source_repo_id: None,
    })
}

fn needle_symbol(path: &str) -> Result<SymbolRecord, Box<dyn Error>> {
    Ok(SymbolRecord {
        symbol_id: SymbolId::new("sym-needle"),
        repo_relative_path: RepoRelativePath::new(path),
        language: LanguageCode::new("rust")?,
        symbol_kind: SymbolKindCode::new("function")?,
        symbol_kind_family: Some(SymbolKindFamily::Callable),
        local_name: "needle".into(),
        qualified_name: "crate::needle".into(),
        signature: None,
        visibility: None,
        definition_span: SymbolSpan {
            path: path.into(),
            byte_start: 0,
            byte_end: 6,
            line_start: 1,
            line_end: 1,
        },
        container_qualified_name: Some("crate".into()),
        relationship: SymbolRelationship::Def,
    })
}

fn publish_history_fixture(rt: &TopKRuntime) -> TestResult {
    let fixture = HistoryFixtureSpec {
        applied_at_ms: 13,
        file_path: "src/needle.rs",
        removed_text: "",
        ..HISTORY_LEDGER
    };
    let batch = fixture.build_batch(
        rt.pin.repo_id.clone(),
        rt.pin.revision_id.clone(),
        rt.pin.manifest_generation,
    )?;
    let request = SearchPlaneIngestIpcRequestEnvelope {
        request_id: 1,
        diagnostic_attempt: quanta_index_contract::DiagnosticAttemptFieldV1::Absent,
        payload: stamped_ingest_request(SearchPlaneIngestIpcRequest::PublishHistoryBatch(batch))?,
    };
    let response: SearchPlaneIngestIpcResponseEnvelope =
        send_request(&rt.ingest_socket, &request, rt.io)?;
    if !matches!(&response.payload, SearchPlaneIngestIpcResponse::HistoryReceipt(receipt) if receipt.applied && receipt.durable_sequence > 0)
    {
        return Err(format!("History fixture publication did not commit: {response:?}").into());
    }
    Ok(())
}

/// One activated pair. The two-page cases have two Structural and Dirty rows;
/// the full route table also needs Symbol and History.
fn start_small_fixture(two_page_routes: bool) -> Result<TopKRuntime, Box<dyn Error>> {
    let rt = TopKRuntime::start(Duration::from_secs(30), 16 * 1024 * 1024)?;
    let pin = &rt.pin;
    let mut batch = SearchCorpusBatch::replace_generation(
        pin.repo_id.clone(),
        pin.revision_id.clone(),
        pin.manifest_generation,
        "lex-seal:1",
    )
    .source_event(SourcePublicationEvent {
        stream_id: "e2e-harness-source-v1".into(),
        event_id: "e2e-source-event-1".into(),
        expected_base_event_id: None,
        payload_sha256: [0; 32],
    });
    let mut structural = Vec::new();
    let mut dirty = Vec::new();
    let files = [
        (
            "src/needle.rs",
            "fn needle() { let needle = 1; }",
            "needle",
            1_u64,
            5_u64,
        ),
        (
            "src/other.rs",
            "fn other() { let needle = 2; }",
            "other",
            2_u64,
            7_u64,
        ),
    ];
    for (path, body, name, ordinal, timestamp) in files {
        let id = ChunkId::new(format!("e2e-{ordinal}-{path}"));
        let file = SourceFileKey {
            source_repo_id: pin.repo_id.clone(),
            repo_relative_path: RepoRelativePath::new(path),
        };
        let symbols = if !two_page_routes && ordinal == 1 {
            vec![needle_symbol(path)?]
        } else {
            Vec::new()
        };
        let mut scope = fixture_source_scope_v1(
            file.clone(),
            pin.revision_id.clone(),
            vec![fixture_chunk(id.clone(), path, body, 1, 2)?],
            symbols,
        )?;
        if !two_page_routes && scope.symbols.is_empty() {
            scope.coverage.symbols = SymbolCoverage::Complete { symbol_count: 0 };
        }
        for semantic in semantic_source_scopes_for_chunk_records(&pin.repo_id, &scope.chunks) {
            batch = batch.replace_semantic_scope(
                semantic.scope,
                semantic.scope_digest,
                semantic.sources,
                semantic.cluster_memberships,
            );
        }
        batch = batch.replace_scope(
            scope.coverage,
            scope.source_bytes,
            scope.chunks,
            scope.symbols,
        );
        if two_page_routes || ordinal == 1 {
            structural.push(StructuralIngestBatch {
                repo_id: pin.repo_id.clone(),
                revision_id: pin.revision_id.clone(),
                generation: pin.manifest_generation,
                base_generation: None,
                manifest_digest: format!("struct:{path}:{}", pin.manifest_generation.get()),
                batch_digest: String::new(),
                mode: BatchIngestMode::Delta,
                replace_scopes: vec![StructuralReplaceScope {
                    scope: SearchScopeKey {
                        doc_surface: SearchScopeSurface::Chunk,
                        source_file: file,
                    },
                    scope_digest: format!("top-k::tree-{ordinal}"),
                    trees: vec![StructuralTreeRecord {
                        chunk_id: id.clone(),
                        record: structural_function_tree_record(path, body, name)?,
                    }],
                }],
                tombstone_scopes: Vec::new(),
                seal: false,
            });
        }
        if two_page_routes || ordinal == 2 {
            dirty.push(
                DirtyBatch::new(
                    pin.repo_id.clone(),
                    pin.revision_id.clone(),
                    pin.manifest_generation,
                    timestamp,
                )
                .upsert(DirtyRecord {
                    wire_version: 1,
                    doc_id: id,
                    applied_at_ms: timestamp,
                    payload_hash: [0x5a; 32],
                }),
            );
        }
    }
    for overlay in dirty {
        let receipt = rt.client.runtime().publish_dirty(&overlay)?;
        if !receipt.applied || receipt.durable_sequence == 0 {
            return Err(format!("top-k Dirty fixture did not commit: {receipt:?}").into());
        }
    }
    if !two_page_routes {
        publish_history_fixture(&rt)?;
    }
    let source = rt.client.search_corpus().publish_outcome(&batch)?;
    if !source.receipt.applied || !source.receipt.sealed || source.receipt.durable_sequence == 0 {
        return Err(format!("top-k fixture did not seal Source: {:?}", source.receipt).into());
    }
    // Source installs the chunk universe. The original per-file Structural
    // deltas populate it before activation without requesting a track seal.
    for batch in structural {
        let receipt = rt.client.structural().publish_wire_batch(batch)?;
        if !receipt.applied
            || receipt.sealed
            || receipt.durable_sequence == 0
            || receipt.generation != pin.manifest_generation
            || receipt.accepted_replace_scopes != 1
        {
            return Err(format!("top-k Structural delta did not apply: {receipt:?}").into());
        }
    }
    let _activation = rt
        .client
        .search_corpus()
        .activate_published(&PublishedBatchEvidence::from(&source), None)?;
    Ok(rt)
}

struct WindowProbe {
    typed_error: Option<SdkError>,
    returned_rows: usize,
    window: Option<QueryResultWindowV2>,
}

fn probe(rt: &TopKRuntime, route: &Route, top_k: u32) -> Result<WindowProbe, String> {
    let result = rt.query((route.build)(Some(rt.pin.clone()), top_k));
    let response = match result {
        Ok(response) => response,
        Err(error @ SdkError::Remote { .. }) => {
            return Ok(WindowProbe {
                typed_error: Some(error),
                returned_rows: 0,
                window: None,
            });
        }
        Err(error) => {
            return Err(format!(
                "{}: top_k={top_k} IPC failure: {error}",
                route.name
            ));
        }
    };
    let (returned_rows, window) = match (route.name, response) {
        ("lexical", SearchPlaneQueryIpcResponse::Text(r)) => (r.results.len(), r.window),
        ("symbol", SearchPlaneQueryIpcResponse::Symbol(r)) => (r.results.len(), r.window),
        ("semantic", SearchPlaneQueryIpcResponse::Semantic(r)) => (r.results.len(), r.window),
        ("hybrid", SearchPlaneQueryIpcResponse::Hybrid(r)) => (r.results.len(), r.window),
        ("hybrid_seed", SearchPlaneQueryIpcResponse::HybridSeed(r)) => {
            (r.seed_candidates.len(), r.window)
        }
        ("history", SearchPlaneQueryIpcResponse::History(r)) => {
            (r.commits.len().saturating_add(r.diffs.len()), r.window)
        }
        ("runtime_metadata", SearchPlaneQueryIpcResponse::RuntimeMetadata(r)) => {
            (r.results.len(), r.window)
        }
        ("structural", SearchPlaneQueryIpcResponse::Structural(r)) => (r.results.len(), r.window),
        (_, other) => {
            return Err(format!(
                "{}: wrong response for top_k={top_k}: {other:?}",
                route.name
            ));
        }
    };
    Ok(WindowProbe {
        typed_error: None,
        returned_rows,
        window: Some(window),
    })
}

fn route_named(name: &str) -> Result<&'static Route, Box<dyn Error>> {
    ROUTES
        .iter()
        .find(|route| route.name == name)
        .ok_or_else(|| format!("truth table has no `{name}` route").into())
}

/// The runtime-metadata and structural routes answer with a window
/// (QI-BB-025 #4).
///
/// At `top_k = 1` over two hits both report `has_more`, structural with the
/// exact count it materialized and runtime with the lower bound its probe
/// saw; at `top_k = 10` both return the two rows with an exact count and no
/// continuation.
#[test]
fn runtime_and_structural_windows_report_the_continuation() -> TestResult {
    let rt = start_small_fixture(true)?;
    let mut failures: Vec<String> = Vec::new();
    for (name, expected_at_one) in [
        ("runtime_metadata", CandidateCountV1::AtLeast(2)),
        ("structural", CandidateCountV1::Exact(2)),
    ] {
        let route = route_named(name)?;
        let page_of_one = probe(&rt, route, 1)?;
        if let Some(error) = page_of_one.typed_error {
            failures.push(format!("{name}: top_k=1 did not serve: {error}"));
            continue;
        }
        match page_of_one.window {
            Some(window)
                if page_of_one.returned_rows == 1
                    && window.returned() == 1
                    && window.has_more() == Some(true)
                    && window.candidate_count() == expected_at_one => {}
            other => failures.push(format!(
                "{name}: top_k=1 over two hits must report one row and has_more with {expected_at_one:?}, got rows={} window={other:?}",
                page_of_one.returned_rows
            )),
        }
        let whole = probe(&rt, route, 10)?;
        if let Some(error) = whole.typed_error {
            failures.push(format!("{name}: top_k=10 did not serve: {error}"));
            continue;
        }
        match whole.window {
            Some(window)
                if whole.returned_rows == 2
                    && window.returned() == 2
                    && window.has_more() == Some(false)
                    && window.candidate_count() == CandidateCountV1::Exact(2) => {}
            other => failures.push(format!(
                "{name}: top_k=10 over two hits must return both with an exact count, got rows={} window={other:?}",
                whole.returned_rows
            )),
        }
    }
    if !failures.is_empty() {
        return Err(format!("route windows drifted:\\n  {}", failures.join("\\n  ")).into());
    }
    rt.stop()
}

/// One keyset route as the continuation check sees it: how to fetch a
/// page after a cursor and how to read the page's rows, window and
/// cursor.
struct KeysetRoute<R> {
    name: &'static str,
    page: fn(&TopKRuntime, Option<ContinuationTokenV2>) -> Result<R, SdkError>,
    rows: fn(&R) -> Vec<String>,
    window: fn(&R) -> QueryResultWindowV2,
    cursor: fn(&R) -> Option<ContinuationTokenV2>,
}

/// Walk `route` at `top_k = 1` over the two-hit fixture and return the
/// failures observed.
///
/// Page one returns exactly one row and an opaque continuation; page two
/// returns the other row without continuation. Together they cover the
/// candidate-id-ordered set exactly once.
fn check_two_page_walk<R>(rt: &TopKRuntime, route: &KeysetRoute<R>) -> Vec<String> {
    let name = route.name;
    let first = match (route.page)(rt, None) {
        Ok(page) => page,
        Err(error) => return vec![format!("{name}: page one did not serve: {error}")],
    };
    let first_rows = (route.rows)(&first);
    let cursor = match (route.cursor)(&first) {
        Some(cursor)
            if first_rows.len() == 1
                && (route.window)(&first).returned() == 1
                && (route.window)(&first).has_more() == Some(true) =>
        {
            cursor
        }
        _ => {
            return vec![format!(
                "{name}: page one of two must return one row and a continuation, got rows {first_rows:?} window {:?}",
                (route.window)(&first)
            )];
        }
    };
    let second = match (route.page)(rt, Some(cursor)) {
        Ok(page) => page,
        Err(error) => return vec![format!("{name}: page two did not serve: {error}")],
    };
    let mut failures = Vec::new();
    let mut walked = first_rows;
    walked.extend((route.rows)(&second));
    let expected = ["e2e-1-src/needle.rs", "e2e-2-src/other.rs"];
    if !walked.iter().map(String::as_str).eq(expected) {
        failures.push(format!(
            "{name}: two pages of one must walk both rows once in order, got {walked:?}"
        ));
    }
    let window = (route.window)(&second);
    if window.returned() != 1
        || window.has_more() != Some(false)
        || (route.cursor)(&second).is_some()
        || window.candidate_count() != CandidateCountV1::Exact(1)
    {
        failures.push(format!(
            "{name}: the last page is exact and final, got {window:?}"
        ));
    }
    failures
}

/// The runtime-metadata and structural routes continue a cut page from
/// its cursor (QI-BB-025 W4).
#[test]
fn runtime_and_structural_cursors_continue_the_page() -> TestResult {
    let rt = start_small_fixture(true)?;
    let mut failures = check_two_page_walk(
        &rt,
        &KeysetRoute {
            name: "runtime_metadata",
            page: |rt, cursor| {
                rt.client
                    .runtime()
                    .query_request(RuntimeMetadataQueryRequest {
                        text_query: text_request(
                            TextQuerySyntax::Sourcegraph,
                            RUNTIME_QUERY,
                            Some(rt.pin.clone()),
                            1,
                        ),
                        cursor,
                    })
            },
            rows: |page| {
                page.results
                    .iter()
                    .map(|row| row.candidate_id.clone())
                    .collect()
            },
            window: |page| page.window.clone(),
            cursor: |page| page.next_cursor.clone(),
        },
    );
    failures.extend(check_two_page_walk(
        &rt,
        &KeysetRoute {
            name: "structural",
            page: |rt, cursor| {
                rt.client
                    .structural()
                    .query_request(StructuralQueryRequest {
                        text_query: text_request(
                            TextQuerySyntax::Native,
                            STRUCTURAL_QUERY,
                            Some(rt.pin.clone()),
                            1,
                        ),
                        cursor,
                    })
            },
            rows: |page| {
                page.results
                    .iter()
                    .map(|row| row.candidate_id.clone())
                    .collect()
            },
            window: |page| page.window.clone(),
            cursor: |page| page.next_cursor.clone(),
        },
    ));
    if !failures.is_empty() {
        return Err(format!("cursor continuation drifted:\n  {}", failures.join("\n  ")).into());
    }
    rt.stop()
}

/// The wire window must agree with the rows actually returned.
///
/// It must not claim a continuation that the row count contradicts. This is
/// an independent check on the values as they crossed the wire, not on the
/// constructor that produced them.
fn window_contradiction(
    window: &QueryResultWindowV2,
    returned_rows: usize,
    top_k: u32,
) -> Option<String> {
    let returned = u64::from(window.returned());
    let Ok(rows) = u64::try_from(returned_rows) else {
        return Some(format!(
            "{returned_rows} returned rows do not fit the wire count"
        ));
    };
    if returned != rows {
        return Some(format!(
            "window.returned={returned} but {rows} rows were returned"
        ));
    }
    let lower_bound = window.candidate_count().lower_bound();
    if lower_bound < returned {
        return Some(format!(
            "candidate_count lower bound {lower_bound} is below returned {returned}"
        ));
    }
    match (window.candidate_count(), window.has_more()) {
        (CandidateCountV1::Exact(exact), Some(false)) if exact == returned => None,
        (CandidateCountV1::Exact(exact), Some(true))
            if exact > returned && returned == u64::from(top_k) =>
        {
            None
        }
        (CandidateCountV1::AtLeast(lower), Some(true))
            if lower > returned && returned == u64::from(top_k) =>
        {
            None
        }
        (CandidateCountV1::AtLeast(_), None) => None,
        (count, has_more) => Some(format!(
            "candidate_count={count:?} has_more={has_more:?} contradict returned={returned} top_k={top_k}"
        )),
    }
}

/// Replace every `top_k` entry in a decoded CBOR tree with `top_k`, so the
/// bytes carry a value no encoder would emit.
#[expect(
    clippy::wildcard_enum_match_arm,
    reason = "ciborium::Value is #[non_exhaustive]; the leaves and any future variant carry no top_k"
)]
fn patch_every_top_k(value: &mut ciborium::Value, top_k: u32) -> usize {
    match value {
        ciborium::Value::Map(fields) => fields
            .iter_mut()
            .map(|(key, entry)| {
                if matches!(key, ciborium::Value::Text(name) if name == "top_k") {
                    *entry = ciborium::Value::Integer(top_k.into());
                    1
                } else {
                    patch_every_top_k(entry, top_k)
                }
            })
            .sum(),
        ciborium::Value::Array(items) => items
            .iter_mut()
            .map(|item| patch_every_top_k(item, top_k))
            .sum(),
        // Every leaf variant, and — `ciborium::Value` being
        // `#[non_exhaustive]` — any variant this codec version does not
        // name, carries no `top_k`.
        _ => 0,
    }
}

/// The daemon's query-plane decode-failure and dispatch counters.
///
/// Wait for all query connections to close before reading the control scrape:
/// one-shot clients receive the answer before the server finishes its next
/// read and decrements its live-connection gauge, so a previous connection's
/// close may otherwise land inside the next request's counter interval.
fn ipc_query_counters(rt: &TopKRuntime) -> Result<(u64, u64), Box<dyn Error>> {
    let deadline = Instant::now()
        .checked_add(Duration::from_secs(5))
        .ok_or("query-counter wait deadline overflowed")?;
    let snapshot = loop {
        let snapshot = rt.client.observability().metrics_snapshot()?;
        let live = snapshot
            .gauges
            .iter()
            .find(|gauge| gauge.name == "ipc_query_connections_live")
            .ok_or("gauge `ipc_query_connections_live` is in the scrape")?
            .value;
        if live.to_bits() == 0.0_f64.to_bits() {
            // The first scrape may have sampled counters before the gauge
            // reached zero. Read counters again after observing quiescence.
            break rt.client.observability().metrics_snapshot()?;
        }
        if Instant::now() >= deadline {
            return Err(format!("query connections did not quiesce: {live} live").into());
        }
        std::thread::sleep(Duration::from_millis(10));
    };
    let counter = |name: &str| -> Result<u64, Box<dyn Error>> {
        snapshot
            .counters
            .iter()
            .find(|counter| counter.name == name)
            .map(|counter| counter.value)
            .ok_or_else(|| format!("counter `{name}` is in the scrape").into())
    };
    Ok((
        counter("ipc_query_request_decode_failures_total")?,
        counter("ipc_query_requests_dispatched_total")?,
    ))
}

/// Every route refuses an out-of-range `top_k` under one code, from the
/// typed client and from raw bytes alike (QI-BB-025 완료 기준: the SDK and
/// raw IPC return the same error code).
///
/// The typed request does not encode, under the shared code; raw bytes
/// carrying the value decode, are dispatched once, and come back as a typed
/// answer with that same code — never served, never a decode failure, never
/// a closed connection that names nothing.
#[test]
fn every_route_refuses_out_of_range_top_k_with_one_code_from_typed_and_raw_callers() -> TestResult {
    let rt = start_small_fixture(false)?;
    // Prime the route once so the pin and the sockets exist and the
    // counters are live before the refusals are counted.
    let lexical = route_named("lexical")?;
    let primed = probe(&rt, lexical, 1)?;
    if primed.typed_error.is_some() {
        return Err(format!(
            "the fixture serves before refusals: {:?}",
            primed.typed_error
        )
        .into());
    }
    let query_socket = &rt.query_socket;
    let pin = &rt.pin;
    let mut failures: Vec<String> = Vec::new();
    for route in &ROUTES {
        for top_k in REFUSED {
            // The client side: the typed request refuses to encode under
            // the shared code, so no SDK or harness caller can emit it.
            let typed = quanta_index_contract::SearchPlaneQueryIpcRequestEnvelope {
                diagnostic_attempt: quanta_index_contract::DiagnosticAttemptFieldV1::Absent,
                request_id: 1,
                payload: (route.build)(Some(pin.clone()), top_k),
            };
            match encode_request(&typed) {
                Ok(_) => failures.push(format!(
                    "{}: top_k={top_k} encoded although it is out of range",
                    route.name
                )),
                Err(err) if err.to_string().contains(TOP_K_OUT_OF_RANGE_CODE) => {}
                Err(err) => failures.push(format!(
                    "{}: top_k={top_k} encode refusal does not name {TOP_K_OUT_OF_RANGE_CODE}: {err}",
                    route.name
                )),
            }
            // The daemon side: bytes no encoder produced, carrying the
            // value in every `top_k` position the route has.
            let in_range = quanta_index_contract::SearchPlaneQueryIpcRequestEnvelope {
                diagnostic_attempt: quanta_index_contract::DiagnosticAttemptFieldV1::Absent,
                request_id: 1,
                payload: (route.build)(Some(pin.clone()), 1),
            };
            let mut wire: ciborium::Value = {
                let mut buf = Vec::new();
                ciborium::into_writer(&in_range, &mut buf)?;
                ciborium::from_reader(buf.as_slice())?
            };
            if patch_every_top_k(&mut wire, top_k) == 0 {
                failures.push(format!("{}: the wire carries no top_k", route.name));
                continue;
            }
            let (failures_before, dispatched_before) = ipc_query_counters(&rt)?;
            let answer = send_request::<
                ciborium::Value,
                quanta_index_contract::SearchPlaneQueryIpcResponseEnvelope,
            >(query_socket, &wire, rt.io);
            match answer.map(|response| response.payload) {
                Ok(quanta_index_contract::SearchPlaneQueryIpcResponse::Error(error))
                    if error.code.as_wire_str() == TOP_K_OUT_OF_RANGE_CODE => {}
                Ok(other) => failures.push(format!(
                    "{}: top_k={top_k} raw bytes must be answered typed {TOP_K_OUT_OF_RANGE_CODE}, got {other:?}",
                    route.name
                )),
                Err(err) => failures.push(format!(
                    "{}: top_k={top_k} raw bytes must be answered, not dropped: {err}",
                    route.name
                )),
            }
            // Decoded and dispatched: the refusal is the dispatcher's, the
            // same validator the encoder and the SDK run.
            let (failures_after, dispatched_after) = ipc_query_counters(&rt)?;
            if failures_after != failures_before {
                failures.push(format!(
                    "{}: top_k={top_k} must not be a decode failure: {failures_before} -> {failures_after}",
                    route.name
                ));
            }
            if dispatched_after != dispatched_before.saturating_add(1) {
                failures.push(format!(
                    "{}: top_k={top_k} must be dispatched exactly once: {dispatched_before} -> {dispatched_after}",
                    route.name
                ));
            }
        }
    }
    if !failures.is_empty() {
        return Err(format!("top_k refusal drifted:\n  {}", failures.join("\n  ")).into());
    }
    rt.stop()
}

/// Every route accepts the public range, including the public maximum, and
/// answers exactly as pinned for this fixture.
#[test]
fn every_route_accepts_the_public_range_including_the_maximum() -> TestResult {
    let rt = start_small_fixture(false)?;
    let mut failures: Vec<String> = Vec::new();
    for route in &ROUTES {
        for top_k in ACCEPTED {
            let observed = match probe(&rt, route, top_k) {
                Ok(observed) => observed,
                Err(failure) => {
                    failures.push(failure);
                    continue;
                }
            };
            match observed.typed_error {
                Some(SdkError::Remote { code, message, .. })
                    if code.as_wire_str() == TOP_K_OUT_OF_RANGE_CODE =>
                {
                    failures.push(format!(
                        "{}: top_k={top_k} is inside the public range but was refused: {message}",
                        route.name
                    ));
                }
                Some(error) => failures.push(format!(
                    "{}: top_k={top_k} must serve on this fixture but answered {error}",
                    route.name
                )),
                None => {
                    if usize::try_from(top_k).is_ok_and(|cap| observed.returned_rows > cap) {
                        failures.push(format!(
                            "{}: top_k={top_k} returned {} rows, more than requested",
                            route.name, observed.returned_rows
                        ));
                    }
                    if let Some(window) = observed.window
                        && let Some(contradiction) =
                            window_contradiction(&window, observed.returned_rows, top_k)
                    {
                        failures.push(format!(
                            "{}: top_k={top_k} window contradiction: {contradiction}",
                            route.name
                        ));
                    }
                }
            }
        }
    }
    if !failures.is_empty() {
        return Err(format!("top_k acceptance drifted:\n  {}", failures.join("\n  ")).into());
    }
    rt.stop()
}

/// The fixture must give every route something to return.
///
/// The acceptance table only proves a route applied the gate *and then
/// served* if the route had rows to serve. Every route must return at least
/// one row at the public maximum, so an empty answer cannot masquerade as
/// acceptance; and the lexical route, which has two hits, must report the
/// continuation at `top_k = 1`.
#[test]
fn fixture_gives_every_route_at_least_one_row() -> TestResult {
    let rt = start_small_fixture(false)?;
    let mut failures: Vec<String> = Vec::new();
    for route in &ROUTES {
        let observed = match probe(&rt, route, PUBLIC_TOP_K_MAX) {
            Ok(observed) => observed,
            Err(failure) => {
                failures.push(failure);
                continue;
            }
        };
        match observed.typed_error {
            Some(error) => failures.push(format!("{}: did not serve: {error}", route.name)),
            None if observed.returned_rows == 0 => {
                failures.push(format!(
                    "{}: served zero rows; fixture is blind",
                    route.name
                ));
            }
            None => {}
        }
    }
    if !failures.is_empty() {
        return Err(format!("fixture is not load-bearing:\n  {}", failures.join("\n  ")).into());
    }
    let lexical = ROUTES
        .iter()
        .find(|route| route.name == "lexical")
        .ok_or("truth table has no lexical route")?;
    let observed = probe(&rt, lexical, 1)?;
    if observed.returned_rows != 1 {
        return Err(format!(
            "lexical route returned {} rows for top_k=1 on a two-hit fixture",
            observed.returned_rows
        )
        .into());
    }
    let window = observed
        .window
        .ok_or("lexical route answered without a wire window")?;
    if window.has_more() != Some(true) {
        return Err("two-hit fixture at top_k=1 must report has_more".into());
    }
    rt.stop()
}

/// Rows holding the needle beyond the public maximum: one more than a page
/// at `top_k = 10_000` can hold.
const OVER_MAXIMUM_ROWS: u32 = PUBLIC_TOP_K_MAX + 1;

/// Rows per file of the over-maximum fixture: eleven files hold them all.
const OVER_MAXIMUM_ROWS_PER_FILE: usize = 910;

/// Publish eleven files in one Source seal, using the SDK's bounded transport.
fn populate_over_maximum_runtime(rt: &TopKRuntime) -> Result<BTreeSet<String>, Box<dyn Error>> {
    let contents: Vec<String> = (0..OVER_MAXIMUM_ROWS)
        .map(|index| format!("let row_{index} = {index}; // needle"))
        .collect();
    let files: Vec<_> = contents.chunks(OVER_MAXIMUM_ROWS_PER_FILE).collect();
    let mut records = Vec::with_capacity(contents.len());
    let mut ordinal = 1_u64;
    for (file_index, file) in files.iter().enumerate() {
        let path = format!("src/needles_{file_index:02}.rs");
        for (line, body) in (1_u32..).zip(file.iter()) {
            let id = ChunkId::new(format!("e2e-{ordinal}-{path}"));
            records.push(fixture_chunk(id, &path, body, line, line)?);
            ordinal = ordinal
                .checked_add(1)
                .ok_or("fixture chunk ordinal exhausted")?;
        }
    }
    let expected_ids: BTreeSet<String> = records
        .iter()
        .map(|record| record.chunk_id.as_str().to_owned())
        .collect();
    if records.len() != usize::try_from(OVER_MAXIMUM_ROWS)? || expected_ids.len() != records.len() {
        return Err(format!(
            "over-maximum fixture staged {} rows with {} distinct IDs",
            records.len(),
            expected_ids.len()
        )
        .into());
    }
    let batch = legacy_source_replacement_from_chunks(
        &rt.pin.repo_id,
        &rt.pin.revision_id,
        rt.pin.manifest_generation.get(),
        records,
    )?;
    let (_head, receipt) = publish_legacy_source_batch(&rt.client, &batch, None)?;
    if !receipt.sealed
        || usize::try_from(receipt.accepted_replace_scopes)? != files.len()
        || receipt.accepted_semantic_replace_scopes != OVER_MAXIMUM_ROWS
    {
        return Err(
            format!("over-maximum Source did not seal all files and rows: {receipt:?}").into(),
        );
    }
    Ok(expected_ids)
}

/// At the public maximum over more rows than a page holds, the page says
/// more exist and the continuation reaches every row (QI-BB-025 완료 기준
/// #3, the `has_more` branch).
///
/// Over 10,001 rows holding the needle, the lexical and semantic routes
/// at `top_k = 10_000` each return exactly 10,000 rows with a window that
/// says more exist and agrees with itself. Walking the lexical cursors at
/// the same `top_k` visits every one of the 10,001 rows exactly once, every
/// page within `top_k` and consistent with its window, and the last page
/// says nothing more exists and names no cursor.
#[test]
fn the_public_maximum_reports_the_continuation_over_ten_thousand_and_one_rows() -> TestResult {
    let rt = TopKRuntime::start(Duration::from_secs(600), 256 * 1024 * 1024)?;
    let scenario_result = populate_over_maximum_runtime(&rt)
        .and_then(|expected| check_public_maximum_continuation(&rt, &expected));
    let teardown_result = rt.stop();
    match (scenario_result, teardown_result) {
        (Ok(()), Ok(())) => Ok(()),
        (Err(scenario), Ok(())) => Err(scenario),
        (Ok(()), Err(teardown)) => Err(format!("daemon teardown failed: {teardown:#}").into()),
        (Err(scenario), Err(teardown)) => Err(format!(
            "scenario failed: {scenario}; daemon teardown also failed: {teardown:#}"
        )
        .into()),
    }
}

fn check_public_maximum_continuation(rt: &TopKRuntime, expected: &BTreeSet<String>) -> TestResult {
    let rows = usize::try_from(OVER_MAXIMUM_ROWS)?;
    let maximum = usize::try_from(PUBLIC_TOP_K_MAX)?;
    for route in ROUTES
        .iter()
        .filter(|route| matches!(route.name, "lexical" | "semantic"))
    {
        let observed = probe(rt, route, PUBLIC_TOP_K_MAX)?;
        if let Some(error) = observed.typed_error {
            return Err(format!("{} at the maximum did not serve: {error}", route.name).into());
        }
        let window = observed
            .window
            .ok_or_else(|| format!("{} answered without a window", route.name))?;
        if observed.returned_rows != maximum || window.has_more() != Some(true) {
            return Err(format!(
                "{} returned {} rows with has_more={:?} over {rows} matching rows",
                route.name,
                observed.returned_rows,
                window.has_more()
            )
            .into());
        }
        if let Some(contradiction) =
            window_contradiction(&window, observed.returned_rows, PUBLIC_TOP_K_MAX)
        {
            return Err(format!("{}: {contradiction}", route.name).into());
        }
    }

    let mut seen = BTreeSet::new();
    let mut cursor = None;
    let mut pages = 0_u32;
    loop {
        let mut request = native_request(Some(rt.pin.clone()), PUBLIC_TOP_K_MAX);
        request.cursor = cursor;
        let page = rt.client.lexical().query_request(request)?;
        if page.generation != rt.pin {
            return Err(format!(
                "cursor walk selected a different pin: {:?}",
                page.generation
            )
            .into());
        }
        pages = pages.saturating_add(1);
        if let Some(contradiction) =
            window_contradiction(&page.window, page.results.len(), PUBLIC_TOP_K_MAX)
        {
            return Err(format!("page {pages}: {contradiction}").into());
        }
        for row in &page.results {
            if !seen.insert(row.candidate_id.clone()) {
                return Err(format!("page {pages} repeats {}", row.candidate_id).into());
            }
        }
        match (page.window.has_more(), page.next_cursor) {
            (Some(true), Some(next)) if pages < 2 => cursor = Some(next),
            (Some(false), None) => break,
            (has_more, next) => {
                return Err(format!(
                    "page {pages}: has_more={has_more:?} but the cursor is {next:?}"
                )
                .into());
            }
        }
    }
    if &seen != expected || pages != 2 {
        let missing = expected.difference(&seen).take(4).collect::<Vec<_>>();
        let unexpected = seen.difference(expected).take(4).collect::<Vec<_>>();
        return Err(format!(
            "the walk saw {} IDs over {pages} pages for {rows} fixture rows, missing={missing:?}, unexpected={unexpected:?}",
            seen.len()
        )
        .into());
    }
    Ok(())
}
