//! E2E-00 — reusable tempdir-backed runtime harness.
//!
//! Parent harness for E2E-01..07. Owns a `TempDir` plus a lazily-started
//! searchd driver thread so a single test can: publish typed ingest batches
//! through the real ingest UDS frontdoor, seal a generation, drop+reopen
//! the runtime, then issue public query IPC requests and read typed responses
//! back. No in-memory shortcut: every byte goes through the same public daemon
//! surfaces the production runtime exposes.
//!
//! The harness intentionally does not add any new public API to
//! `quanta-index-searchd-runtime`. `reopen` is implemented by dropping
//! the current driver thread and reconstructing a fresh runtime over
//! the same `state_root`.

#![expect(
    dead_code,
    reason = "harness API surface is consumed across E2E-00..07; only the inventory smoke exercises a slice today"
)]

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::thread;
use std::time::{Duration, Instant};

use anyhow::Result as AnyResult;
use quanta_index_contract::lex::{
    LanguageCode, ParseNode, ParseTreeRecord, SymbolKindCode, SymbolKindFamily, SymbolRecord,
    SymbolRelationship, SymbolSpan, compute_parse_tree_source_hash,
};
use quanta_index_contract::{
    BatchIngestMode, ChunkId, ChunkRecord, EmbeddingId, EmbeddingRecord, EngineTouched,
    GenerationPin, HybridQueryRequest, LexicalCandidate, LexicalIngestBatch, LexicalReplaceScope,
    LexicalTombstoneScope, ManifestGeneration, OwnerDocKind, RepoId, RepoRelativePath, RevisionId,
    SearchExplanation, SearchPlaneActivateGenerationRequest, SearchPlaneExplainQueryRequest,
    SearchPlaneIngestIpcRequest, SearchPlaneIngestIpcRequestEnvelope, SearchPlaneIngestIpcResponse,
    SearchPlaneIngestIpcResponseEnvelope, SearchPlaneQueryIpcRequest,
    SearchPlaneQueryIpcRequestEnvelope, SearchPlaneQueryIpcResponse,
    SearchPlaneQueryIpcResponseEnvelope, SearchPlaneTrackKind, SemanticIngestBatch,
    SemanticQueryRequest, SemanticReplaceScope, StructuralIngestBatch, StructuralQueryRequest,
    StructuralReplaceScope, StructuralTreeRecord, SymbolId, TextQueryRequest, TextQuerySyntax,
};
use quanta_index_ipc::send_request;
use quanta_index_search_plane::ActivationCatalog;
use quanta_index_search_plane::{BoundedQueryObsStore, MetricSample, ObsError};
use quanta_index_searchd::app::SearchdConfig;
use quanta_index_searchd::app::searchd::drive;
use quanta_index_searchd_runtime::build_runtime;
use tempfile::TempDir;

static NEXT_SOCKET_ID: AtomicU64 = AtomicU64::new(0);
const READINESS_TIMEOUT: Duration = Duration::from_secs(15);
const SOCKET_APPEAR_TIMEOUT: Duration = Duration::from_secs(5);

type DriverJoin = thread::JoinHandle<AnyResult<()>>;

/// Tempdir-backed runtime handle.
#[expect(
    clippy::redundant_pub_crate,
    reason = "sibling test modules import this private-module harness surface"
)]
pub(super) struct E2eRuntime {
    tempdir: Option<TempDir>,
    state_root: PathBuf,
    driver: Option<DriverState>,
    query_obs_store: Option<Arc<BoundedQueryObsStore>>,
    chunk_ids_by_path: BTreeMap<String, ChunkId>,
    chunk_records_by_path: BTreeMap<String, ChunkRecord>,
    request_id_counter: AtomicU64,
    generation_counter: u64,
}

struct DriverState {
    query_socket: PathBuf,
    ingest_socket: PathBuf,
    shutdown: Arc<AtomicBool>,
    join: Option<DriverJoin>,
}

/// Test-only response shape.
///
/// Carries either the candidate list or a typed error code.
/// `engines_touched` is best-effort; the `Text` response variant has no
/// explanation today so this stays empty for plain text queries. Semantic and
/// hybrid rows populate it in later E2E tickets.
#[expect(
    clippy::redundant_pub_crate,
    reason = "sibling test modules import this private-module harness surface"
)]
pub(super) struct E2eQueryResult {
    pub(super) candidates: Vec<LexicalCandidate>,
    pub(super) candidate_ids: Vec<String>,
    pub(super) engines_touched: Vec<EngineTouched>,
    pub(super) explanation: Option<SearchExplanation>,
    pub(super) typed_error: Option<E2eTypedError>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
#[expect(
    clippy::redundant_pub_crate,
    reason = "sibling test modules import this private-module harness surface"
)]
pub(super) struct E2eTypedError {
    pub(super) code: String,
    pub(super) message: String,
}

#[expect(
    clippy::redundant_pub_crate,
    reason = "sibling test modules import this private-module harness surface"
)]
pub(super) struct E2eExplainResult {
    pub(super) explanation: Option<SearchExplanation>,
    pub(super) typed_error: Option<E2eTypedError>,
}

impl E2eRuntime {
    /// Create a fresh tempdir and an owned publisher. The driver is NOT
    /// started yet — it boots lazily on first `query_text`.
    pub(super) fn boot() -> AnyResult<Self> {
        let tempdir = tempfile::tempdir()?;
        let state_root = tempdir.path().to_path_buf();
        Ok(Self {
            tempdir: Some(tempdir),
            state_root,
            driver: None,
            query_obs_store: None,
            chunk_ids_by_path: BTreeMap::new(),
            chunk_records_by_path: BTreeMap::new(),
            request_id_counter: AtomicU64::new(1),
            generation_counter: 1,
        })
    }

    /// Stop the driver (if running) and reconstruct a publisher over the
    /// same `state_root` so further ingest is possible, then leave the
    /// driver stopped so first query lazy-starts a fresh runtime.
    /// Mirrors a process restart against persistent storage.
    pub(super) fn reopen(mut self) -> AnyResult<Self> {
        self.stop_driver();
        Ok(self)
    }

    fn stop_driver(&mut self) {
        if let Some(mut driver) = self.driver.take() {
            driver.shutdown.store(true, Ordering::Release);
            if let Some(join) = driver.join.take() {
                drop(join.join());
            }
        }
        self.query_obs_store = None;
    }

    fn ensure_driver(&mut self) -> AnyResult<PathBuf> {
        if self.driver.is_none() {
            let (query_socket, ingest_socket, shutdown, join, query_obs_store) =
                start_driver(&self.state_root)?;
            self.query_obs_store = Some(Arc::clone(&query_obs_store));
            self.driver = Some(DriverState {
                query_socket,
                ingest_socket,
                shutdown,
                join: Some(join),
            });
        }
        self.driver
            .as_ref()
            .map(|driver| driver.query_socket.clone())
            .ok_or_else(|| anyhow::anyhow!("e2e-harness: driver missing after ensure_driver"))
    }

    fn ensure_ingest_socket(&mut self) -> AnyResult<PathBuf> {
        drop(self.ensure_driver()?);
        self.driver
            .as_ref()
            .map(|driver| driver.ingest_socket.clone())
            .ok_or_else(|| {
                anyhow::anyhow!("e2e-harness: ingest socket missing after ensure_driver")
            })
    }

    pub(super) fn repo(&self) -> RepoId {
        RepoId::new("repo-e2e")
    }

    pub(super) fn revision(&self) -> RevisionId {
        RevisionId::new("rev-e2e")
    }

    /// Current generation pin. Stable until `seal()` is called, then
    /// advances on the next ingest.
    pub(super) fn current_generation(&self) -> ManifestGeneration {
        ManifestGeneration::new(self.generation_counter)
    }

    pub(super) fn generation_pin(&self) -> GenerationPin {
        GenerationPin::new(self.repo(), self.revision(), self.current_generation())
    }

    pub(super) fn query_metrics_snapshot(&self) -> AnyResult<Vec<MetricSample>> {
        let store = self.query_obs_store.as_ref().ok_or_else(|| {
            anyhow::anyhow!("e2e-harness: query metrics unavailable before driver startup")
        })?;
        Ok(store.snapshot())
    }

    pub(super) fn query_metric_errors(&self) -> AnyResult<Vec<ObsError>> {
        let store = self.query_obs_store.as_ref().ok_or_else(|| {
            anyhow::anyhow!("e2e-harness: query metrics unavailable before driver startup")
        })?;
        Ok(store.errors())
    }

    pub(super) fn activate_last_sealed_generation(&self) -> AnyResult<()> {
        self.activate_last_sealed_generation_with_tracks(&[SearchPlaneTrackKind::Lexical])
    }

    pub(super) fn activate_last_sealed_generation_with_tracks(
        &self,
        tracks: &[SearchPlaneTrackKind],
    ) -> AnyResult<()> {
        let Some(pin) = self.last_sealed_pin() else {
            return Err(anyhow::anyhow!(
                "e2e-harness: cannot activate before any generation has been sealed"
            ));
        };
        let catalog = ActivationCatalog::open(self.state_root.join("activations"))?;
        catalog.activate(&SearchPlaneActivateGenerationRequest {
            repo_id: pin.repo_id,
            revision_id: pin.revision_id,
            manifest_generation: pin.manifest_generation,
            manifest_digest: "e2e-harness-activation".to_string(),
            tracks: tracks.to_vec(),
        })?;
        Ok(())
    }

    /// Ingest one chunk through the typed ingest front door.
    ///
    /// `_repo` is informational metadata only — the publish itself goes
    /// against the harness's owning `repo()` so the matching query can
    /// pin to a stable triple.
    pub(super) fn ingest_text(&mut self, _repo: &str, path: &str, content: &str) -> AnyResult<()> {
        let chunk_id = ChunkId::new(format!(
            "e2e-{}-{path}",
            self.request_id_counter.fetch_add(1, Ordering::Relaxed)
        ));
        let record = ChunkRecord {
            chunk_id: chunk_id.clone(),
            repo_relative_path: RepoRelativePath::new(path),
            language: LanguageCode::new(language_from_path(path)).map_err(|err| {
                anyhow::anyhow!("language_from_path must return canonical lowercase codes: {err}")
            })?,
            start_byte: 0,
            end_byte: u32::try_from(content.len())
                .map_err(|err| anyhow::anyhow!("e2e harness content length overflow: {err}"))?,
            start_line: 1,
            end_line: 2,
            snippet: content.to_string().into_boxed_str(),
            indexed_text: content.to_string().into_boxed_str(),
            text_digest: format!("text:{path}:{content}").into_boxed_str(),
            shape_digest: format!("shape:{path}:{content}").into_boxed_str(),
            structural: None,
            parent_chunk_id: None,
        };
        self.dispatch_ingest(SearchPlaneIngestIpcRequest::PublishLexicalBatch(
            LexicalIngestBatch {
                repo_id: self.repo(),
                revision_id: self.revision(),
                generation: self.current_generation(),
                base_generation: None,
                manifest_digest: format!("lex:{path}:{}", self.current_generation().get()),
                batch_digest: format!(
                    "lex-batch:{path}:{}",
                    self.request_id_counter.load(Ordering::Relaxed)
                ),
                mode: BatchIngestMode::Delta,
                bundle_payload: None,
                replace_scopes: vec![LexicalReplaceScope {
                    scope: scope_key(path),
                    scope_digest: format!("scope:{path}:{content}"),
                    chunks: vec![record.clone()],
                    symbols: Vec::new(),
                }],
                tombstone_scopes: Vec::new(),
                seal: false,
            },
        ))?;
        let _old = self
            .chunk_ids_by_path
            .insert(path.to_string(), record.chunk_id.clone());
        let _old = self.chunk_records_by_path.insert(path.to_string(), record);
        Ok(())
    }

    pub(super) fn ingest_semantic_embedding_for_path(
        &mut self,
        path: &str,
        vector: &[f32],
    ) -> AnyResult<()> {
        let chunk_id = self.chunk_ids_by_path.get(path).cloned().ok_or_else(|| {
            anyhow::anyhow!("e2e-harness: no lexical chunk recorded for semantic path `{path}`")
        })?;
        let chunk = self
            .chunk_records_by_path
            .get(path)
            .cloned()
            .ok_or_else(|| {
                anyhow::anyhow!(
                    "e2e-harness: no lexical chunk payload recorded for semantic path `{path}`"
                )
            })?;
        self.dispatch_ingest(SearchPlaneIngestIpcRequest::PublishSemanticBatch(
            SemanticIngestBatch {
                repo_id: self.repo(),
                revision_id: self.revision(),
                generation: self.current_generation(),
                base_generation: None,
                manifest_digest: format!("sem:{path}:{}", self.current_generation().get()),
                batch_digest: format!(
                    "sem-batch:{path}:{}",
                    self.request_id_counter.load(Ordering::Relaxed)
                ),
                mode: BatchIngestMode::Delta,
                model_contract: semantic_model_contract(vector.len())?,
                replace_scopes: vec![SemanticReplaceScope {
                    scope: scope_key(path),
                    scope_digest: format!("sem-scope:{path}"),
                    embeddings: vec![EmbeddingRecord {
                        embedding_id: EmbeddingId::new(chunk_id.as_str()),
                        owner_kind: OwnerDocKind::Chunk,
                        owner_id: chunk_id.as_str().to_string().into_boxed_str(),
                        source_doc_id: chunk_id.as_str().to_string().into_boxed_str(),
                        repo_relative_path: RepoRelativePath::new(path),
                        language: chunk.language,
                        symbol_kind: None,
                        start_byte: chunk.start_byte,
                        end_byte: chunk.end_byte,
                        start_line: chunk.start_line,
                        end_line: chunk.end_line,
                        snippet: chunk.snippet.clone(),
                        embedding_input_digest: format!("embed-in:{path}:{vector:?}")
                            .into_boxed_str(),
                        vector_digest: format!("embed-vec:{path}:{vector:?}").into_boxed_str(),
                        view_kind: "raw_chunk".to_string().into_boxed_str(),
                        vector: vector.to_vec(),
                    }],
                }],
                tombstone_scopes: Vec::new(),
                seal: false,
            },
        ))?;
        Ok(())
    }

    pub(super) fn ingest_structural_function_tree(
        &mut self,
        path: &str,
        content: &str,
        identifier: &str,
    ) -> AnyResult<()> {
        let chunk_id = self.chunk_ids_by_path.get(path).cloned().ok_or_else(|| {
            anyhow::anyhow!("e2e-harness: no lexical chunk recorded for structural path `{path}`")
        })?;
        let identifier_start = content.find(identifier).ok_or_else(|| {
            anyhow::anyhow!(
                "e2e-harness: identifier `{identifier}` not present in structural content"
            )
        })?;
        let identifier_end = identifier_start.saturating_add(identifier.len());
        let byte_end = u32::try_from(content.len())
            .map_err(|err| anyhow::anyhow!("e2e harness structural content overflow: {err}"))?;
        let identifier_start = u32::try_from(identifier_start)
            .map_err(|err| anyhow::anyhow!("e2e harness identifier start overflow: {err}"))?;
        let identifier_end = u32::try_from(identifier_end)
            .map_err(|err| anyhow::anyhow!("e2e harness identifier end overflow: {err}"))?;
        let tree = ParseTreeRecord {
            wire_version: 1,
            lang: LanguageCode::new(language_from_path(path)).map_err(|err| {
                anyhow::anyhow!("language_from_path must return canonical lowercase codes: {err}")
            })?,
            root: ParseNode {
                kind: "function_item".to_string().into_boxed_str(),
                byte_start: 0,
                byte_end,
                children: vec![
                    ParseNode {
                        kind: "identifier".to_string().into_boxed_str(),
                        byte_start: identifier_start,
                        byte_end: identifier_end,
                        children: Vec::new(),
                    },
                    ParseNode {
                        kind: "block".to_string().into_boxed_str(),
                        byte_start: byte_end.saturating_sub(2),
                        byte_end,
                        children: Vec::new(),
                    },
                ],
            },
            source_hash: compute_parse_tree_source_hash(content),
            role_tag_schema_version: 1,
            role_tags: Vec::new(),
        };
        self.dispatch_ingest(SearchPlaneIngestIpcRequest::PublishStructuralBatch(
            StructuralIngestBatch {
                repo_id: self.repo(),
                revision_id: self.revision(),
                generation: self.current_generation(),
                base_generation: None,
                manifest_digest: format!("struct:{path}:{}", self.current_generation().get()),
                batch_digest: format!(
                    "struct-batch:{path}:{}",
                    self.request_id_counter.load(Ordering::Relaxed)
                ),
                mode: BatchIngestMode::Delta,
                replace_scopes: vec![StructuralReplaceScope {
                    scope: scope_key(path),
                    scope_digest: format!("struct-scope:{path}"),
                    trees: vec![StructuralTreeRecord {
                        chunk_id,
                        record: tree,
                    }],
                }],
                tombstone_scopes: Vec::new(),
                seal: false,
            },
        ))?;
        Ok(())
    }

    pub(super) fn delete_chunk_for_path(&mut self, path: &str) -> AnyResult<()> {
        self.dispatch_ingest(SearchPlaneIngestIpcRequest::PublishLexicalBatch(
            LexicalIngestBatch {
                repo_id: self.repo(),
                revision_id: self.revision(),
                generation: self.current_generation(),
                base_generation: None,
                manifest_digest: format!("lex-del:{path}:{}", self.current_generation().get()),
                batch_digest: format!(
                    "lex-del-batch:{path}:{}",
                    self.request_id_counter.load(Ordering::Relaxed)
                ),
                mode: BatchIngestMode::Delta,
                bundle_payload: None,
                replace_scopes: Vec::new(),
                tombstone_scopes: vec![LexicalTombstoneScope {
                    scope: scope_key(path),
                }],
                seal: false,
            },
        ))?;
        drop(self.chunk_records_by_path.remove(path));
        Ok(())
    }

    pub(super) fn ingest_symbol(
        &mut self,
        _repo: &str,
        path: &str,
        symbol_id: &str,
        symbol_name: &str,
    ) -> AnyResult<()> {
        let record = SymbolRecord {
            symbol_id: SymbolId::new(symbol_id),
            repo_relative_path: RepoRelativePath::new(path),
            language: LanguageCode::new(language_from_path(path)).map_err(|err| {
                anyhow::anyhow!("language_from_path must return canonical lowercase codes: {err}")
            })?,
            symbol_kind: SymbolKindCode::new("function")
                .map_err(|err| anyhow::anyhow!("invalid test symbol kind: {err}"))?,
            symbol_kind_family: Some(SymbolKindFamily::Callable),
            local_name: symbol_name.to_string().into_boxed_str(),
            qualified_name: format!("crate::{symbol_name}").into_boxed_str(),
            signature: None,
            visibility: None,
            definition_span: SymbolSpan {
                path: path.to_string().into_boxed_str(),
                byte_start: 0,
                byte_end: u32::try_from(symbol_name.len())
                    .map_err(|err| anyhow::anyhow!("e2e harness symbol length overflow: {err}"))?,
                line_start: 1,
                line_end: 1,
            },
            container_qualified_name: Some("crate".to_string().into_boxed_str()),
            relationship: SymbolRelationship::Def,
        };
        let chunks = self
            .chunk_records_by_path
            .get(path)
            .cloned()
            .into_iter()
            .collect::<Vec<_>>();
        self.dispatch_ingest(SearchPlaneIngestIpcRequest::PublishLexicalBatch(
            LexicalIngestBatch {
                repo_id: self.repo(),
                revision_id: self.revision(),
                generation: self.current_generation(),
                base_generation: None,
                manifest_digest: format!("lex-symbol:{path}:{}", self.current_generation().get()),
                batch_digest: format!("lex-symbol-batch:{path}:{symbol_id}"),
                mode: BatchIngestMode::Delta,
                bundle_payload: None,
                replace_scopes: vec![LexicalReplaceScope {
                    scope: scope_key(path),
                    scope_digest: format!("scope-symbol:{path}:{symbol_id}"),
                    chunks,
                    symbols: vec![record],
                }],
                tombstone_scopes: Vec::new(),
                seal: false,
            },
        ))?;
        Ok(())
    }

    /// Seal the current generation. Returns the sealed `ManifestGeneration`
    /// then advances the harness's pin so subsequent ingests target the
    /// next generation.
    pub(super) fn seal(&mut self) -> AnyResult<ManifestGeneration> {
        self.seal_tracks(&[SearchPlaneTrackKind::Lexical])
    }

    pub(super) fn seal_tracks(
        &mut self,
        tracks: &[SearchPlaneTrackKind],
    ) -> AnyResult<ManifestGeneration> {
        let sealed = self.current_generation();
        if tracks.contains(&SearchPlaneTrackKind::Lexical) {
            self.dispatch_ingest(SearchPlaneIngestIpcRequest::PublishLexicalBatch(
                LexicalIngestBatch {
                    repo_id: self.repo(),
                    revision_id: self.revision(),
                    generation: sealed,
                    base_generation: None,
                    manifest_digest: format!("lex-seal:{}", sealed.get()),
                    batch_digest: format!("lex-seal-batch:{}", sealed.get()),
                    mode: BatchIngestMode::Delta,
                    bundle_payload: None,
                    replace_scopes: Vec::new(),
                    tombstone_scopes: Vec::new(),
                    seal: true,
                },
            ))?;
        }
        if tracks.contains(&SearchPlaneTrackKind::Semantic) {
            self.dispatch_ingest(SearchPlaneIngestIpcRequest::PublishSemanticBatch(
                SemanticIngestBatch {
                    repo_id: self.repo(),
                    revision_id: self.revision(),
                    generation: sealed,
                    base_generation: None,
                    manifest_digest: format!("sem-seal:{}", sealed.get()),
                    batch_digest: format!("sem-seal-batch:{}", sealed.get()),
                    mode: BatchIngestMode::Delta,
                    model_contract: semantic_model_contract(1)?,
                    replace_scopes: Vec::new(),
                    tombstone_scopes: Vec::new(),
                    seal: true,
                },
            ))?;
        }
        self.generation_counter = self.generation_counter.saturating_add(1);
        Ok(sealed)
    }

    /// Issue a `TextQueryRequest` against the live socket, lazy-starting
    /// the driver on first call. The query is pinned to whatever
    /// generation was most recently sealed (i.e. `current_generation() -
    /// 1`), matching how production callers pin queries to a sealed
    /// manifest.
    pub(super) fn query_text(
        &mut self,
        syntax: TextQuerySyntax,
        query_text: &str,
        top_k: u32,
    ) -> E2eQueryResult {
        let pin = self.last_sealed_pin();
        self.query_text_with_pin(syntax, query_text, top_k, pin)
    }

    pub(super) fn query_structural(
        &mut self,
        syntax: TextQuerySyntax,
        query_text: &str,
        top_k: u32,
    ) -> E2eQueryResult {
        let pin = self.last_sealed_pin();
        self.query_structural_with_pin(syntax, query_text, top_k, pin)
    }

    /// Variant that lets a self-test exercise the "no generation pin"
    /// invalid-contract path explicitly.
    pub(super) fn query_text_with_pin(
        &mut self,
        syntax: TextQuerySyntax,
        query_text: &str,
        top_k: u32,
        pin: Option<GenerationPin>,
    ) -> E2eQueryResult {
        let request_id = self.request_id_counter.fetch_add(1, Ordering::Relaxed);
        let envelope = SearchPlaneQueryIpcRequestEnvelope {
            request_id,
            payload: SearchPlaneQueryIpcRequest::Text(TextQueryRequest {
                syntax,
                query_text: query_text.to_string(),
                generation: pin,
                generation_selector: None,
                top_k,
            }),
        };
        let socket = match self.ensure_driver() {
            Ok(socket) => socket,
            Err(err) => {
                return E2eQueryResult {
                    candidates: Vec::new(),
                    candidate_ids: Vec::new(),
                    engines_touched: Vec::new(),
                    explanation: None,
                    typed_error: Some(E2eTypedError {
                        code: "HARNESS_START".to_string(),
                        message: err.to_string(),
                    }),
                };
            }
        };
        // Wait for the runtime to leave NOT_READY before reading the real
        // payload — mirrors the readiness wait every existing dsl_scenarios
        // test does. For an invalid-contract request (no pin), the runtime
        // returns INVALID_REQUEST immediately, which already satisfies the
        // "non-NOT_READY" predicate.
        let readiness_reached = wait_until(READINESS_TIMEOUT, || {
            match send_request::<_, SearchPlaneQueryIpcResponseEnvelope>(&socket, &envelope) {
                Ok(response) => match &response.payload {
                    SearchPlaneQueryIpcResponse::Error(err) => {
                        err.code != "NOT_READY" && err.code != "STR_GENERATION_NOT_READY"
                    }
                    SearchPlaneQueryIpcResponse::Text(_)
                    | SearchPlaneQueryIpcResponse::Symbol(_)
                    | SearchPlaneQueryIpcResponse::Semantic(_)
                    | SearchPlaneQueryIpcResponse::Hybrid(_)
                    | SearchPlaneQueryIpcResponse::History(_)
                    | SearchPlaneQueryIpcResponse::Structural(_)
                    | SearchPlaneQueryIpcResponse::Bridge(_)
                    | SearchPlaneQueryIpcResponse::RepoMapQuery(_)
                    | SearchPlaneQueryIpcResponse::Explain(_)
                    | quanta_index_contract::SearchPlaneQueryIpcResponse::RuntimeMetadata(_) => {
                        true
                    }
                },
                Err(_transport_error) => false,
            }
        });
        let response: SearchPlaneQueryIpcResponseEnvelope = match send_request(&socket, &envelope) {
            Ok(r) => r,
            Err(err) => return self.semantic_transport_error(readiness_reached, err),
        };
        match response.payload {
            SearchPlaneQueryIpcResponse::Text(text) => E2eQueryResult {
                candidate_ids: text
                    .results
                    .iter()
                    .map(|c| c.candidate_id.clone())
                    .collect(),
                candidates: text.results,
                engines_touched: Vec::new(),
                explanation: None,
                typed_error: None,
            },
            SearchPlaneQueryIpcResponse::Error(err) => E2eQueryResult {
                candidates: Vec::new(),
                candidate_ids: Vec::new(),
                engines_touched: Vec::new(),
                explanation: None,
                typed_error: Some(E2eTypedError {
                    code: err.code,
                    message: err.message,
                }),
            },
            SearchPlaneQueryIpcResponse::Symbol(_) => unexpected_response("Symbol"),
            SearchPlaneQueryIpcResponse::Semantic(_) => unexpected_response("Semantic"),
            SearchPlaneQueryIpcResponse::Hybrid(_) => unexpected_response("Hybrid"),
            SearchPlaneQueryIpcResponse::History(_) => unexpected_response("History"),
            SearchPlaneQueryIpcResponse::Structural(_) => unexpected_response("Structural"),
            SearchPlaneQueryIpcResponse::Bridge(_) => unexpected_response("Bridge"),
            SearchPlaneQueryIpcResponse::RepoMapQuery(_) => unexpected_response("RepoMapQuery"),
            SearchPlaneQueryIpcResponse::Explain(_) => unexpected_response("Explain"),
            SearchPlaneQueryIpcResponse::RuntimeMetadata(_) => {
                unexpected_response("RuntimeMetadata")
            }
        }
    }

    pub(super) fn query_structural_with_pin(
        &mut self,
        syntax: TextQuerySyntax,
        query_text: &str,
        top_k: u32,
        pin: Option<GenerationPin>,
    ) -> E2eQueryResult {
        let request_id = self.request_id_counter.fetch_add(1, Ordering::Relaxed);
        let envelope = SearchPlaneQueryIpcRequestEnvelope {
            request_id,
            payload: SearchPlaneQueryIpcRequest::Structural(StructuralQueryRequest {
                text_query: TextQueryRequest {
                    syntax,
                    query_text: query_text.to_string(),
                    generation: pin,
                    generation_selector: None,
                    top_k,
                },
            }),
        };
        let socket = match self.ensure_driver() {
            Ok(socket) => socket,
            Err(err) => {
                return E2eQueryResult {
                    candidates: Vec::new(),
                    candidate_ids: Vec::new(),
                    engines_touched: Vec::new(),
                    explanation: None,
                    typed_error: Some(E2eTypedError {
                        code: "HARNESS_START".to_string(),
                        message: err.to_string(),
                    }),
                };
            }
        };
        let readiness_reached = wait_until(READINESS_TIMEOUT, || {
            match send_request::<_, SearchPlaneQueryIpcResponseEnvelope>(&socket, &envelope) {
                Ok(response) => match &response.payload {
                    SearchPlaneQueryIpcResponse::Error(err) => {
                        err.code != "NOT_READY" && err.code != "STR_GENERATION_NOT_READY"
                    }
                    SearchPlaneQueryIpcResponse::Text(_)
                    | SearchPlaneQueryIpcResponse::Symbol(_)
                    | SearchPlaneQueryIpcResponse::Semantic(_)
                    | SearchPlaneQueryIpcResponse::Hybrid(_)
                    | SearchPlaneQueryIpcResponse::History(_)
                    | SearchPlaneQueryIpcResponse::Structural(_)
                    | SearchPlaneQueryIpcResponse::Bridge(_)
                    | SearchPlaneQueryIpcResponse::RepoMapQuery(_)
                    | SearchPlaneQueryIpcResponse::Explain(_)
                    | quanta_index_contract::SearchPlaneQueryIpcResponse::RuntimeMetadata(_) => {
                        true
                    }
                },
                Err(_transport_error) => false,
            }
        });
        let response: SearchPlaneQueryIpcResponseEnvelope = match send_request(&socket, &envelope) {
            Ok(r) => r,
            Err(err) => return self.semantic_transport_error(readiness_reached, err),
        };
        match response.payload {
            SearchPlaneQueryIpcResponse::Structural(structural) => E2eQueryResult {
                candidates: Vec::new(),
                candidate_ids: structural
                    .results
                    .into_iter()
                    .map(|candidate| candidate.candidate_id)
                    .collect(),
                engines_touched: Vec::new(),
                explanation: None,
                typed_error: None,
            },
            SearchPlaneQueryIpcResponse::Error(err) => E2eQueryResult {
                candidates: Vec::new(),
                candidate_ids: Vec::new(),
                engines_touched: Vec::new(),
                explanation: None,
                typed_error: Some(E2eTypedError {
                    code: err.code,
                    message: err.message,
                }),
            },
            SearchPlaneQueryIpcResponse::Text(_) => unexpected_response("Text"),
            SearchPlaneQueryIpcResponse::Symbol(_) => unexpected_response("Symbol"),
            SearchPlaneQueryIpcResponse::Semantic(_) => unexpected_response("Semantic"),
            SearchPlaneQueryIpcResponse::Hybrid(_) => unexpected_response("Hybrid"),
            SearchPlaneQueryIpcResponse::History(_) => unexpected_response("History"),
            SearchPlaneQueryIpcResponse::Bridge(_) => unexpected_response("Bridge"),
            SearchPlaneQueryIpcResponse::RepoMapQuery(_) => unexpected_response("RepoMapQuery"),
            SearchPlaneQueryIpcResponse::Explain(_) => unexpected_response("Explain"),
            SearchPlaneQueryIpcResponse::RuntimeMetadata(_) => {
                unexpected_response("RuntimeMetadata")
            }
        }
    }

    pub(super) fn query_semantic(
        &mut self,
        vector: &[f32],
        top_k: u32,
        lexical_scope: Option<(TextQuerySyntax, &str, u32)>,
    ) -> E2eQueryResult {
        let request_id = self.request_id_counter.fetch_add(1, Ordering::Relaxed);
        let envelope = SearchPlaneQueryIpcRequestEnvelope {
            request_id,
            payload: SearchPlaneQueryIpcRequest::Semantic(SemanticQueryRequest {
                query_text: float_vec_to_query_text(vector),
                generation: self.last_sealed_pin(),
                generation_selector: None,
                lexical_scope: lexical_scope.map(|(syntax, query_text, scope_top_k)| {
                    TextQueryRequest {
                        syntax,
                        query_text: query_text.to_string(),
                        generation: self.last_sealed_pin(),
                        generation_selector: None,
                        top_k: scope_top_k,
                    }
                }),
                top_k,
            }),
        };
        let socket = match self.ensure_driver() {
            Ok(socket) => socket,
            Err(err) => {
                return E2eQueryResult {
                    candidates: Vec::new(),
                    candidate_ids: Vec::new(),
                    engines_touched: Vec::new(),
                    explanation: None,
                    typed_error: Some(E2eTypedError {
                        code: "HARNESS_START".to_string(),
                        message: err.to_string(),
                    }),
                };
            }
        };
        let readiness_reached = wait_until(READINESS_TIMEOUT, || {
            match send_request::<_, SearchPlaneQueryIpcResponseEnvelope>(&socket, &envelope) {
                Ok(response) => match &response.payload {
                    SearchPlaneQueryIpcResponse::Error(err) => err.code != "NOT_READY",
                    SearchPlaneQueryIpcResponse::Text(_)
                    | SearchPlaneQueryIpcResponse::Symbol(_)
                    | SearchPlaneQueryIpcResponse::Semantic(_)
                    | SearchPlaneQueryIpcResponse::Hybrid(_)
                    | SearchPlaneQueryIpcResponse::History(_)
                    | SearchPlaneQueryIpcResponse::Structural(_)
                    | SearchPlaneQueryIpcResponse::Bridge(_)
                    | SearchPlaneQueryIpcResponse::RepoMapQuery(_)
                    | SearchPlaneQueryIpcResponse::Explain(_)
                    | quanta_index_contract::SearchPlaneQueryIpcResponse::RuntimeMetadata(_) => {
                        true
                    }
                },
                Err(_transport_error) => false,
            }
        });
        let response: SearchPlaneQueryIpcResponseEnvelope = match send_request(&socket, &envelope) {
            Ok(r) => r,
            Err(err) => return self.semantic_transport_error(readiness_reached, err),
        };
        match response.payload {
            SearchPlaneQueryIpcResponse::Semantic(semantic) => E2eQueryResult {
                candidate_ids: semantic
                    .results
                    .iter()
                    .map(|c| c.candidate_id.clone())
                    .collect(),
                candidates: semantic.results,
                engines_touched: semantic.explanation.engines_touched.clone(),
                explanation: Some(semantic.explanation),
                typed_error: None,
            },
            SearchPlaneQueryIpcResponse::Error(err) => E2eQueryResult {
                candidates: Vec::new(),
                candidate_ids: Vec::new(),
                engines_touched: Vec::new(),
                explanation: None,
                typed_error: Some(E2eTypedError {
                    code: err.code,
                    message: err.message,
                }),
            },
            SearchPlaneQueryIpcResponse::Text(_) => unexpected_response("Text"),
            SearchPlaneQueryIpcResponse::Symbol(_) => unexpected_response("Symbol"),
            SearchPlaneQueryIpcResponse::Hybrid(_) => unexpected_response("Hybrid"),
            SearchPlaneQueryIpcResponse::History(_) => unexpected_response("History"),
            SearchPlaneQueryIpcResponse::Structural(_) => unexpected_response("Structural"),
            SearchPlaneQueryIpcResponse::Bridge(_) => unexpected_response("Bridge"),
            SearchPlaneQueryIpcResponse::RepoMapQuery(_) => unexpected_response("RepoMapQuery"),
            SearchPlaneQueryIpcResponse::Explain(_) => unexpected_response("Explain"),
            SearchPlaneQueryIpcResponse::RuntimeMetadata(_) => {
                unexpected_response("RuntimeMetadata")
            }
        }
    }

    pub(super) fn query_hybrid(
        &mut self,
        syntax: TextQuerySyntax,
        text_query: &str,
        vector: &[f32],
        top_k: u32,
    ) -> E2eQueryResult {
        let pin = self.last_sealed_pin();
        let request_id = self.request_id_counter.fetch_add(1, Ordering::Relaxed);
        let envelope = SearchPlaneQueryIpcRequestEnvelope {
            request_id,
            payload: SearchPlaneQueryIpcRequest::Hybrid(HybridQueryRequest {
                text_query: TextQueryRequest {
                    syntax,
                    query_text: text_query.to_string(),
                    generation: pin.clone(),
                    generation_selector: None,
                    top_k: 50,
                },
                semantic_query_text: float_vec_to_query_text(vector),
                generation: pin,
                generation_selector: None,
                top_k,
            }),
        };
        let socket = match self.ensure_driver() {
            Ok(socket) => socket,
            Err(err) => {
                return E2eQueryResult {
                    candidates: Vec::new(),
                    candidate_ids: Vec::new(),
                    engines_touched: Vec::new(),
                    explanation: None,
                    typed_error: Some(E2eTypedError {
                        code: "HARNESS_START".to_string(),
                        message: err.to_string(),
                    }),
                };
            }
        };
        let readiness_reached = wait_until(READINESS_TIMEOUT, || {
            match send_request::<_, SearchPlaneQueryIpcResponseEnvelope>(&socket, &envelope) {
                Ok(response) => match &response.payload {
                    SearchPlaneQueryIpcResponse::Error(err) => err.code != "NOT_READY",
                    SearchPlaneQueryIpcResponse::Text(_)
                    | SearchPlaneQueryIpcResponse::Symbol(_)
                    | SearchPlaneQueryIpcResponse::Semantic(_)
                    | SearchPlaneQueryIpcResponse::Hybrid(_)
                    | SearchPlaneQueryIpcResponse::History(_)
                    | SearchPlaneQueryIpcResponse::Structural(_)
                    | SearchPlaneQueryIpcResponse::Bridge(_)
                    | SearchPlaneQueryIpcResponse::RepoMapQuery(_)
                    | SearchPlaneQueryIpcResponse::Explain(_)
                    | quanta_index_contract::SearchPlaneQueryIpcResponse::RuntimeMetadata(_) => {
                        true
                    }
                },
                Err(_transport_error) => false,
            }
        });
        let response: SearchPlaneQueryIpcResponseEnvelope = match send_request(&socket, &envelope) {
            Ok(r) => r,
            Err(err) => return self.semantic_transport_error(readiness_reached, err),
        };
        match response.payload {
            SearchPlaneQueryIpcResponse::Hybrid(hybrid) => E2eQueryResult {
                candidate_ids: hybrid
                    .results
                    .iter()
                    .map(|c| c.candidate_id.clone())
                    .collect(),
                candidates: hybrid.results,
                engines_touched: hybrid.explanation.engines_touched.clone(),
                explanation: Some(hybrid.explanation),
                typed_error: None,
            },
            SearchPlaneQueryIpcResponse::Error(err) => E2eQueryResult {
                candidates: Vec::new(),
                candidate_ids: Vec::new(),
                engines_touched: Vec::new(),
                explanation: None,
                typed_error: Some(E2eTypedError {
                    code: err.code,
                    message: err.message,
                }),
            },
            SearchPlaneQueryIpcResponse::Text(_) => unexpected_response("Text"),
            SearchPlaneQueryIpcResponse::Symbol(_) => unexpected_response("Symbol"),
            SearchPlaneQueryIpcResponse::Semantic(_) => unexpected_response("Semantic"),
            SearchPlaneQueryIpcResponse::History(_) => unexpected_response("History"),
            SearchPlaneQueryIpcResponse::Structural(_) => unexpected_response("Structural"),
            SearchPlaneQueryIpcResponse::Bridge(_) => unexpected_response("Bridge"),
            SearchPlaneQueryIpcResponse::RepoMapQuery(_) => unexpected_response("RepoMapQuery"),
            SearchPlaneQueryIpcResponse::Explain(_) => unexpected_response("Explain"),
            SearchPlaneQueryIpcResponse::RuntimeMetadata(_) => {
                unexpected_response("RuntimeMetadata")
            }
        }
    }

    pub(super) fn candidate_id_for_path(&self, path: &str) -> AnyResult<String> {
        self.chunk_ids_by_path
            .get(path)
            .map(|chunk_id| chunk_id.as_str().to_string())
            .ok_or_else(|| anyhow::anyhow!("e2e-harness: no chunk id recorded for path `{path}`"))
    }

    fn semantic_transport_error(
        &mut self,
        readiness_reached: bool,
        err: impl std::fmt::Display,
    ) -> E2eQueryResult {
        let mut message = if readiness_reached {
            err.to_string()
        } else {
            format!("readiness timeout before IPC response: {err}")
        };
        if let Some(mut driver) = self.driver.take() {
            driver.shutdown.store(true, Ordering::Release);
            if let Some(join) = driver.join.take() {
                match join.join() {
                    Ok(Ok(())) => message.push_str("; driver exited cleanly before response"),
                    Ok(Err(driver_err)) => {
                        message.push_str("; driver exited with error: ");
                        message.push_str(&driver_err.to_string());
                    }
                    Err(panic) => {
                        let panic_message = format!("{panic:?}");
                        message.push_str("; driver panicked: ");
                        message.push_str(&panic_message);
                    }
                }
            }
        }
        E2eQueryResult {
            candidates: Vec::new(),
            candidate_ids: Vec::new(),
            engines_touched: Vec::new(),
            explanation: None,
            typed_error: Some(E2eTypedError {
                code: "IPC_TRANSPORT".to_string(),
                message,
            }),
        }
    }

    pub(super) fn explain_candidate(&mut self, candidate: LexicalCandidate) -> E2eExplainResult {
        let request_id = self.request_id_counter.fetch_add(1, Ordering::Relaxed);
        let pin = GenerationPin::new(
            candidate.repo_id.clone(),
            candidate.revision_id.clone(),
            candidate.manifest_generation,
        );
        let envelope = SearchPlaneQueryIpcRequestEnvelope {
            request_id,
            payload: SearchPlaneQueryIpcRequest::Explain(SearchPlaneExplainQueryRequest {
                generation: pin,
                candidate,
            }),
        };
        let socket = match self.ensure_driver() {
            Ok(socket) => socket,
            Err(err) => {
                return E2eExplainResult {
                    explanation: None,
                    typed_error: Some(E2eTypedError {
                        code: "HARNESS_START".to_string(),
                        message: err.to_string(),
                    }),
                };
            }
        };
        let readiness_reached = wait_until(READINESS_TIMEOUT, || {
            match send_request::<_, SearchPlaneQueryIpcResponseEnvelope>(&socket, &envelope) {
                Ok(response) => match &response.payload {
                    SearchPlaneQueryIpcResponse::Error(err) => err.code != "NOT_READY",
                    SearchPlaneQueryIpcResponse::Text(_)
                    | SearchPlaneQueryIpcResponse::Symbol(_)
                    | SearchPlaneQueryIpcResponse::Semantic(_)
                    | SearchPlaneQueryIpcResponse::Hybrid(_)
                    | SearchPlaneQueryIpcResponse::History(_)
                    | SearchPlaneQueryIpcResponse::Structural(_)
                    | SearchPlaneQueryIpcResponse::Bridge(_)
                    | SearchPlaneQueryIpcResponse::RepoMapQuery(_)
                    | SearchPlaneQueryIpcResponse::Explain(_)
                    | quanta_index_contract::SearchPlaneQueryIpcResponse::RuntimeMetadata(_) => {
                        true
                    }
                },
                Err(_transport_error) => false,
            }
        });
        let response: SearchPlaneQueryIpcResponseEnvelope = match send_request(&socket, &envelope) {
            Ok(r) => r,
            Err(err) => {
                let message = if readiness_reached {
                    err.to_string()
                } else {
                    format!("readiness timeout before IPC response: {err}")
                };
                return E2eExplainResult {
                    explanation: None,
                    typed_error: Some(E2eTypedError {
                        code: "IPC_TRANSPORT".to_string(),
                        message,
                    }),
                };
            }
        };
        match response.payload {
            SearchPlaneQueryIpcResponse::Explain(explain) => E2eExplainResult {
                explanation: Some(explain.explanation),
                typed_error: None,
            },
            SearchPlaneQueryIpcResponse::Error(err) => E2eExplainResult {
                explanation: None,
                typed_error: Some(E2eTypedError {
                    code: err.code,
                    message: err.message,
                }),
            },
            SearchPlaneQueryIpcResponse::Text(_) => unexpected_explain_response("Text"),
            SearchPlaneQueryIpcResponse::Symbol(_) => unexpected_explain_response("Symbol"),
            SearchPlaneQueryIpcResponse::Semantic(_) => unexpected_explain_response("Semantic"),
            SearchPlaneQueryIpcResponse::Hybrid(_) => unexpected_explain_response("Hybrid"),
            SearchPlaneQueryIpcResponse::History(_) => unexpected_explain_response("History"),
            SearchPlaneQueryIpcResponse::Structural(_) => unexpected_explain_response("Structural"),
            SearchPlaneQueryIpcResponse::Bridge(_) => unexpected_explain_response("Bridge"),
            SearchPlaneQueryIpcResponse::RepoMapQuery(_) => {
                unexpected_explain_response("RepoMapQuery")
            }
            SearchPlaneQueryIpcResponse::RuntimeMetadata(_) => {
                unexpected_explain_response("RuntimeMetadata")
            }
        }
    }

    fn last_sealed_pin(&self) -> Option<GenerationPin> {
        if self.generation_counter <= 1 {
            None
        } else {
            Some(GenerationPin::new(
                self.repo(),
                self.revision(),
                ManifestGeneration::new(self.generation_counter.saturating_sub(1)),
            ))
        }
    }

    fn dispatch_ingest(&mut self, payload: SearchPlaneIngestIpcRequest) -> AnyResult<()> {
        let socket = self.ensure_ingest_socket()?;
        let request_id = self.request_id_counter.fetch_add(1, Ordering::Relaxed);
        let envelope = SearchPlaneIngestIpcRequestEnvelope {
            request_id,
            payload,
        };
        let response: SearchPlaneIngestIpcResponseEnvelope = send_request(&socket, &envelope)?;
        match response.payload {
            SearchPlaneIngestIpcResponse::LexicalReceipt(_)
            | SearchPlaneIngestIpcResponse::SemanticReceipt(_)
            | SearchPlaneIngestIpcResponse::HistoryReceipt(_)
            | SearchPlaneIngestIpcResponse::DirtyReceipt(_)
            | SearchPlaneIngestIpcResponse::StructuralReceipt(_)
            | SearchPlaneIngestIpcResponse::RepoMapReceipt(_) => Ok(()),
            SearchPlaneIngestIpcResponse::Error(err) => Err(anyhow::anyhow!(
                "e2e-harness ingest failed code={} message={}",
                err.code,
                err.message
            )),
        }
    }
}

fn unexpected_explain_response(kind: &str) -> E2eExplainResult {
    E2eExplainResult {
        explanation: None,
        typed_error: Some(E2eTypedError {
            code: "UNEXPECTED_RESPONSE".to_string(),
            message: format!("expected Explain, got {kind}"),
        }),
    }
}

impl Drop for E2eRuntime {
    fn drop(&mut self) {
        self.stop_driver();
        drop(self.tempdir.take());
    }
}

fn start_driver(
    state_root: &Path,
) -> AnyResult<(
    PathBuf,
    PathBuf,
    Arc<AtomicBool>,
    DriverJoin,
    Arc<BoundedQueryObsStore>,
)> {
    let config = build_config(state_root);
    let runtime = build_runtime(config)?;
    let query_socket = runtime.query_server.socket_path().to_path_buf();
    let ingest_socket = runtime.ingest_server.socket_path().to_path_buf();
    let query_obs_store = Arc::clone(&runtime.query_obs_store);
    let shutdown = Arc::new(AtomicBool::new(false));
    let shutdown_for_drive = Arc::clone(&shutdown);
    let join = thread::Builder::new()
        .name("e2e-harness-driver".into())
        .spawn(move || drive(runtime, shutdown_for_drive))?;
    if !wait_until(SOCKET_APPEAR_TIMEOUT, || {
        query_socket.exists() && ingest_socket.exists()
    }) {
        shutdown.store(true, Ordering::Release);
        drop(join.join());
        return Err(anyhow::anyhow!(
            "e2e-harness: sockets never appeared query={} ingest={}",
            query_socket.display(),
            ingest_socket.display()
        ));
    }
    Ok((query_socket, ingest_socket, shutdown, join, query_obs_store))
}

fn build_config(state_root: &Path) -> SearchdConfig {
    let mut cfg = SearchdConfig::from_state_root(state_root.to_path_buf());
    let (query_socket, control_socket, ingest_socket) = unique_socket_paths();
    cfg = SearchdConfig::with_socket_overrides(cfg, query_socket, control_socket);
    SearchdConfig::with_ingest_socket_override(cfg, ingest_socket)
}

fn unexpected_response(kind: &str) -> E2eQueryResult {
    E2eQueryResult {
        candidates: Vec::new(),
        candidate_ids: Vec::new(),
        engines_touched: Vec::new(),
        explanation: None,
        typed_error: Some(E2eTypedError {
            code: "UNEXPECTED_RESPONSE".to_string(),
            message: format!("expected Text, got {kind}"),
        }),
    }
}

fn unique_socket_paths() -> (PathBuf, PathBuf, PathBuf) {
    let pid = std::process::id();
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |duration| duration.as_nanos());
    let sequence = NEXT_SOCKET_ID.fetch_add(1, Ordering::Relaxed);
    let query = std::env::temp_dir().join(format!("qi-e2e-query-{pid}-{nanos}-{sequence}.sock"));
    let control =
        std::env::temp_dir().join(format!("qi-e2e-control-{pid}-{nanos}-{sequence}.sock"));
    let ingest = std::env::temp_dir().join(format!("qi-e2e-ingest-{pid}-{nanos}-{sequence}.sock"));
    (query, control, ingest)
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

fn language_from_path(path: &str) -> &'static str {
    match path.rsplit('.').next() {
        Some("rs") => "rust",
        Some("py") => "python",
        Some("ts") => "typescript",
        Some("js") => "javascript",
        Some("md") => "markdown",
        Some(_) | None => "text",
    }
}

fn scope_key(path: &str) -> quanta_index_contract::SearchScopeKey {
    quanta_index_contract::SearchScopeKey {
        doc_surface: quanta_index_contract::SearchScopeSurface::Chunk,
        repo_relative_path: RepoRelativePath::new(path),
    }
}

fn semantic_model_contract(
    dimension: usize,
) -> AnyResult<quanta_index_contract::EmbeddingModelContract> {
    let dimension = u32::try_from(dimension)
        .map_err(|err| anyhow::anyhow!("semantic model contract dimension overflow: {err}"))?;
    if dimension == 0 {
        return Err(anyhow::anyhow!(
            "semantic model contract dimension must be non-zero"
        ));
    }
    Ok(quanta_index_contract::EmbeddingModelContract {
        model_id: "e2e-harness-model".to_string().into_boxed_str(),
        model_version: None,
        dimension,
        normalization: quanta_index_contract::EmbeddingNormalization::None,
        distance_metric: quanta_index_contract::EmbeddingDistanceMetric::Cosine,
        policy_digest: "policy:e2e-harness".to_string().into_boxed_str(),
        view_policy_digest: None,
    })
}

fn float_vec_to_bytes(vec: &[f32]) -> AnyResult<Vec<u8>> {
    let owned: Vec<f32> = vec.to_vec();
    let mut out = Vec::new();
    ciborium::into_writer(&owned, &mut out)
        .map_err(|err| anyhow::anyhow!("ciborium encode embedding: {err}"))?;
    Ok(out)
}

fn float_vec_to_query_text(vec: &[f32]) -> String {
    vec.iter()
        .map(std::string::ToString::to_string)
        .collect::<Vec<_>>()
        .join(" ")
}
