//! E2E-00 — reusable tempdir-backed runtime harness.
//!
//! Parent harness for E2E-01..07. Owns a `TempDir` plus a lazily-started
//! searchd driver thread so a single test can: write records through the
//! real `LexicalChannelOp` publish path, seal a generation, drop+reopen
//! the runtime, then issue public `TextQueryRequest`s through the IPC
//! socket and read typed responses back. No in-memory shortcut: every
//! byte goes through the same wire surface the production daemon uses.
//!
//! Lifecycle constraint:
//!
//! The lexical WAL publisher is a single-writer file lock. The running
//! searchd runtime holds that lock for its entire lifetime. That means a
//! test cannot publish chunks while the driver is running. The harness
//! therefore keeps a single owned publisher alive across `ingest_text` /
//! `seal` calls, and lazily starts the driver thread on the first
//! `query_text` (releasing the publisher first). `reopen` tears the
//! driver down, leaves the publisher dropped, restarts the driver against
//! the same `state_root`.
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
use ciborium::into_writer;
use quanta_index_channel::{
    BundleChannelPublisher, LexicalWalPublisher, SemanticWalPublisher, open_lexical_publisher,
    open_semantic_publisher,
};
use quanta_index_contract::lex::{
    LanguageCode, ParseNode, ParseTreeRecord, SymbolKindCode, SymbolKindFamily, SymbolRecord,
    SymbolRelationship, SymbolSpan, compute_parse_tree_source_hash,
};
use quanta_index_contract::{
    ChunkId, ChunkRecord, DeleteChunk, EmbeddingId, EngineTouched, GenerationPin,
    HybridQueryRequest, LexicalCandidate, LexicalChannelOp, ManifestGeneration, RepoId,
    RepoRelativePath, RevisionId, SearchExplanation, SearchPlaneActivateGenerationRequest,
    SearchPlaneExplainQueryRequest, SearchPlaneQueryIpcRequest, SearchPlaneQueryIpcRequestEnvelope,
    SearchPlaneQueryIpcResponse, SearchPlaneQueryIpcResponseEnvelope, SearchPlaneTrackKind,
    SemanticChannelOp, SemanticFullBundle, SemanticQueryRequest, StructuralQueryRequest, SymbolId,
    TextQueryRequest, TextQuerySyntax, UpsertChunk, UpsertEmbedding, UpsertParseTree, UpsertSymbol,
};
use quanta_index_ipc::send_request;
use quanta_index_search_plane::ActivationCatalog;
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
    /// Owned publisher kept alive during ingest. Dropped before the driver
    /// is started so the runtime can reacquire the WAL lock.
    publisher: Option<LexicalWalPublisher>,
    semantic_publisher: Option<SemanticWalPublisher>,
    driver: Option<DriverState>,
    chunk_ids_by_path: BTreeMap<String, ChunkId>,
    request_id_counter: AtomicU64,
    generation_counter: u64,
    semantic_bundle_generation: Option<ManifestGeneration>,
}

struct DriverState {
    socket: PathBuf,
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
        let publisher = open_lexical_publisher(&state_root)?;
        Ok(Self {
            tempdir: Some(tempdir),
            state_root,
            publisher: Some(publisher),
            semantic_publisher: None,
            driver: None,
            chunk_ids_by_path: BTreeMap::new(),
            request_id_counter: AtomicU64::new(1),
            generation_counter: 1,
            semantic_bundle_generation: None,
        })
    }

    /// Stop the driver (if running) and reconstruct a publisher over the
    /// same `state_root` so further ingest is possible, then leave the
    /// driver stopped so first query lazy-starts a fresh runtime.
    /// Mirrors a process restart against persistent storage.
    pub(super) fn reopen(mut self) -> AnyResult<Self> {
        self.stop_driver();
        if self.publisher.is_none() {
            self.publisher = Some(open_lexical_publisher(&self.state_root)?);
        }
        Ok(self)
    }

    fn stop_driver(&mut self) {
        if let Some(mut driver) = self.driver.take() {
            driver.shutdown.store(true, Ordering::Release);
            if let Some(join) = driver.join.take() {
                drop(join.join());
            }
        }
    }

    fn ensure_driver(&mut self) -> AnyResult<PathBuf> {
        if self.driver.is_none() {
            // Release publisher lock before booting the runtime.
            drop(self.publisher.take());
            drop(self.semantic_publisher.take());
            let (socket, shutdown, join) = start_driver(&self.state_root)?;
            self.driver = Some(DriverState {
                socket,
                shutdown,
                join: Some(join),
            });
        }
        self.driver
            .as_ref()
            .map(|driver| driver.socket.clone())
            .ok_or_else(|| anyhow::anyhow!("e2e-harness: driver missing after ensure_driver"))
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

    /// Ingest one chunk through the real `LexicalChannelOp` publish path.
    ///
    /// `_repo` is informational metadata only — the publish itself goes
    /// against the harness's owning `repo()` so the matching query can
    /// pin to a stable triple.
    ///
    /// Requires the driver to be stopped (no concurrent WAL writer). If
    /// the driver is running, this stops it first.
    pub(super) fn ingest_text(&mut self, _repo: &str, path: &str, content: &str) -> AnyResult<()> {
        if self.driver.is_some() {
            self.stop_driver();
        }
        if self.publisher.is_none() {
            self.publisher = Some(open_lexical_publisher(&self.state_root)?);
        }
        let publisher = self
            .publisher
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("e2e-harness: publisher missing after re-open"))?;
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
        let mut buf: Vec<u8> = Vec::new();
        into_writer(&record, &mut buf)?;
        let _seq = publisher.publish(LexicalChannelOp::UpsertChunk(UpsertChunk {
            repo_id: self.repo(),
            revision_id: self.revision(),
            generation: self.current_generation(),
            chunk_id,
            payload: buf,
        }))?;
        let _old = self
            .chunk_ids_by_path
            .insert(path.to_string(), record.chunk_id);
        Ok(())
    }

    pub(super) fn ingest_semantic_embedding_for_path(
        &mut self,
        path: &str,
        vector: &[f32],
    ) -> AnyResult<()> {
        if self.driver.is_some() {
            self.stop_driver();
        }
        if self.semantic_publisher.is_none() {
            self.semantic_publisher = Some(open_semantic_publisher(&self.state_root)?);
        }
        let publisher = self.semantic_publisher.as_ref().ok_or_else(|| {
            anyhow::anyhow!("e2e-harness: semantic publisher missing after re-open")
        })?;
        let current_generation = self.current_generation();
        if self.semantic_bundle_generation != Some(current_generation) {
            let _seq = publisher.publish(SemanticChannelOp::FullBundle(SemanticFullBundle {
                repo_id: self.repo(),
                revision_id: self.revision(),
                generation: current_generation,
                payload: Vec::new(),
            }))?;
            self.semantic_bundle_generation = Some(current_generation);
        }
        let chunk_id = self.chunk_ids_by_path.get(path).cloned().ok_or_else(|| {
            anyhow::anyhow!("e2e-harness: no lexical chunk recorded for semantic path `{path}`")
        })?;
        let _seq = publisher.publish(SemanticChannelOp::UpsertEmbedding(UpsertEmbedding {
            repo_id: self.repo(),
            revision_id: self.revision(),
            generation: current_generation,
            embedding_id: EmbeddingId::new(chunk_id.as_str()),
            payload: float_vec_to_bytes(vector)?,
        }))?;
        Ok(())
    }

    pub(super) fn ingest_structural_function_tree(
        &mut self,
        path: &str,
        content: &str,
        identifier: &str,
    ) -> AnyResult<()> {
        if self.driver.is_some() {
            self.stop_driver();
        }
        if self.publisher.is_none() {
            self.publisher = Some(open_lexical_publisher(&self.state_root)?);
        }
        let publisher = self
            .publisher
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("e2e-harness: publisher missing after re-open"))?;
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
        let mut buf: Vec<u8> = Vec::new();
        into_writer(&tree, &mut buf)?;
        let _seq = publisher.publish(LexicalChannelOp::UpsertParseTree(UpsertParseTree {
            repo_id: self.repo(),
            revision_id: self.revision(),
            generation: self.current_generation(),
            chunk_id,
            payload: buf,
        }))?;
        Ok(())
    }

    pub(super) fn delete_chunk_for_path(&mut self, path: &str) -> AnyResult<()> {
        if self.driver.is_some() {
            self.stop_driver();
        }
        if self.publisher.is_none() {
            self.publisher = Some(open_lexical_publisher(&self.state_root)?);
        }
        let publisher = self
            .publisher
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("e2e-harness: publisher missing after re-open"))?;
        let chunk_id = self.chunk_ids_by_path.get(path).cloned().ok_or_else(|| {
            anyhow::anyhow!("e2e-harness: no lexical chunk recorded for tombstone path `{path}`")
        })?;
        let _seq = publisher.publish(LexicalChannelOp::DeleteChunk(DeleteChunk {
            repo_id: self.repo(),
            revision_id: self.revision(),
            generation: self.current_generation(),
            chunk_id,
        }))?;
        Ok(())
    }

    pub(super) fn ingest_symbol(
        &mut self,
        _repo: &str,
        path: &str,
        symbol_id: &str,
        symbol_name: &str,
    ) -> AnyResult<()> {
        if self.driver.is_some() {
            self.stop_driver();
        }
        if self.publisher.is_none() {
            self.publisher = Some(open_lexical_publisher(&self.state_root)?);
        }
        let publisher = self
            .publisher
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("e2e-harness: publisher missing after re-open"))?;
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
        let mut buf: Vec<u8> = Vec::new();
        into_writer(&record, &mut buf)?;
        let _seq = publisher.publish(LexicalChannelOp::UpsertSymbol(UpsertSymbol {
            repo_id: self.repo(),
            revision_id: self.revision(),
            generation: self.current_generation(),
            symbol_id: SymbolId::new(symbol_id),
            payload: buf,
        }))?;
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
        if self.driver.is_some() {
            self.stop_driver();
        }
        let sealed = self.current_generation();
        if tracks.contains(&SearchPlaneTrackKind::Lexical) {
            if self.publisher.is_none() {
                self.publisher = Some(open_lexical_publisher(&self.state_root)?);
            }
            let publisher = self.publisher.as_ref().ok_or_else(|| {
                anyhow::anyhow!("e2e-harness: lexical publisher missing for seal")
            })?;
            let _seq = publisher.seal(self.repo(), self.revision(), sealed)?;
        }
        if tracks.contains(&SearchPlaneTrackKind::Semantic) {
            if self.semantic_publisher.is_none() {
                self.semantic_publisher = Some(open_semantic_publisher(&self.state_root)?);
            }
            let publisher = self.semantic_publisher.as_ref().ok_or_else(|| {
                anyhow::anyhow!("e2e-harness: semantic publisher missing for seal")
            })?;
            let _seq = publisher.seal(self.repo(), self.revision(), sealed)?;
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
                query_text: Some(float_vec_to_query_text(vector)),
                query_vector: None,
                query_vector_ref: None,
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
                semantic_query_text: Some(float_vec_to_query_text(vector)),
                semantic_vector: None,
                semantic_vector_ref: None,
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
        // Drop publisher before tempdir so file locks release first.
        drop(self.publisher.take());
        drop(self.semantic_publisher.take());
        drop(self.tempdir.take());
    }
}

fn start_driver(state_root: &Path) -> AnyResult<(PathBuf, Arc<AtomicBool>, DriverJoin)> {
    let config = build_config(state_root);
    let runtime = build_runtime(config)?;
    let socket = runtime.query_server.socket_path().to_path_buf();
    let shutdown = Arc::new(AtomicBool::new(false));
    let shutdown_for_drive = Arc::clone(&shutdown);
    let join = thread::Builder::new()
        .name("e2e-harness-driver".into())
        .spawn(move || drive(runtime, shutdown_for_drive))?;
    if !wait_until(SOCKET_APPEAR_TIMEOUT, || socket.exists()) {
        shutdown.store(true, Ordering::Release);
        drop(join.join());
        return Err(anyhow::anyhow!(
            "e2e-harness: socket {} never appeared",
            socket.display()
        ));
    }
    Ok((socket, shutdown, join))
}

fn build_config(state_root: &Path) -> SearchdConfig {
    let mut cfg = SearchdConfig::from_state_root(state_root.to_path_buf());
    let (query_socket, control_socket) = unique_socket_paths();
    cfg = SearchdConfig::with_socket_overrides(cfg, query_socket, control_socket);
    cfg
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

fn unique_socket_paths() -> (PathBuf, PathBuf) {
    let pid = std::process::id();
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |duration| duration.as_nanos());
    let sequence = NEXT_SOCKET_ID.fetch_add(1, Ordering::Relaxed);
    let query = std::env::temp_dir().join(format!("qi-e2e-query-{pid}-{nanos}-{sequence}.sock"));
    let control =
        std::env::temp_dir().join(format!("qi-e2e-control-{pid}-{nanos}-{sequence}.sock"));
    (query, control)
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
