//! Structural producer test doubles and chunk fixtures.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, RwLock};

use quanta_index_contract::channel::{LexicalChannelOp, UpsertChunk};
use quanta_index_contract::lex::LanguageCode;
use quanta_index_contract::{
    AuxEpochV1, ChunkId, ChunkRecord, ManifestGeneration, RepoId, RepoRelativePath, RevisionId,
    SearchPlaneTrackKind,
};
use quanta_index_core::domains::structural::StructuralQueryRequest as DomainStructuralQueryRequest;
use quanta_index_core::domains::structural::{
    StructuralError, StructuralProducerPort, StructuralQueryRequest, StructuralReadiness,
};
use quanta_index_core::{LexicalIndexOpenPort, StructuralMatchBinding, StructuralMatchCandidate};

use crate::Ledger;
use crate::query_dispatcher::dispatcher::SearchPlaneDispatcher;
use crate::query_dispatcher::tests::support::common::{
    TestResult, encode_cbor, ready_ledger, test_activation_catalog,
};
use crate::query_dispatcher::tests::support::lexical::RejectLexicalOpener;
use crate::query_dispatcher::tests::support::repo_map::StubRepoMapQueryPort;
use crate::query_dispatcher::tests::support::semantic::RejectSemanticOpener;

/// Test-only fail-closed structural producer.
///
/// Production runtime wiring uses the ledger-backed adapter in
/// `searchd::app::runtime`. This stand-in remains only for unit tests that
/// exercise unrelated query surfaces without materializing structural
/// authority.
pub(crate) struct FailClosedStructuralProducer;

impl StructuralProducerPort for FailClosedStructuralProducer {
    fn readiness(
        &self,
        _request: &DomainStructuralQueryRequest,
    ) -> quanta_index_core::domains::structural::StructuralReadiness {
        quanta_index_core::domains::structural::StructuralReadiness::ParseTreeProducerUnavailable
    }

    fn execute(
        &self,
        _request: &DomainStructuralQueryRequest,
    ) -> Result<
        Vec<quanta_index_core::StructuralMatchCandidate>,
        quanta_index_core::domains::structural::StructuralError,
    > {
        Err(quanta_index_core::domains::structural::StructuralError::ProducerExecution(
            "FailClosedStructuralProducer.execute should remain unreachable while readiness is ParseTreeProducerUnavailable".to_string(),
        ))
    }
}

/// Test producer that records how many times `readiness` was consulted
/// and lets a test choose which readiness value is returned.
pub(crate) struct RecordingStructuralProducer {
    pub(crate) readiness: StructuralReadiness,
    pub(crate) results: Vec<StructuralMatchCandidate>,
    pub(crate) execute_error: Option<StructuralError>,
    pub(crate) readiness_calls: AtomicUsize,
    pub(crate) execute_calls: AtomicUsize,
    /// The structural authority epoch each executed request pinned
    /// (QI-BB-020 W2), in call order.
    pub(crate) executed_epochs: Mutex<Vec<AuxEpochV1>>,
}

impl RecordingStructuralProducer {
    pub(crate) fn new(readiness: StructuralReadiness) -> Self {
        Self {
            readiness,
            results: Vec::new(),
            execute_error: None,
            readiness_calls: AtomicUsize::new(0),
            execute_calls: AtomicUsize::new(0),
            executed_epochs: Mutex::new(Vec::new()),
        }
    }

    pub(crate) fn ready_with(results: Vec<StructuralMatchCandidate>) -> Self {
        Self {
            readiness: StructuralReadiness::Ready,
            results,
            execute_error: None,
            readiness_calls: AtomicUsize::new(0),
            execute_calls: AtomicUsize::new(0),
            executed_epochs: Mutex::new(Vec::new()),
        }
    }

    pub(crate) fn ready_with_error(execute_error: StructuralError) -> Self {
        Self {
            readiness: StructuralReadiness::Ready,
            results: Vec::new(),
            execute_error: Some(execute_error),
            readiness_calls: AtomicUsize::new(0),
            execute_calls: AtomicUsize::new(0),
            executed_epochs: Mutex::new(Vec::new()),
        }
    }
}

impl StructuralProducerPort for RecordingStructuralProducer {
    fn readiness(&self, _request: &StructuralQueryRequest) -> StructuralReadiness {
        let _prev: usize = self.readiness_calls.fetch_add(1, Ordering::SeqCst);
        self.readiness.clone()
    }

    fn execute(
        &self,
        request: &StructuralQueryRequest,
    ) -> Result<Vec<StructuralMatchCandidate>, StructuralError> {
        let _prev: usize = self.execute_calls.fetch_add(1, Ordering::SeqCst);
        self.executed_epochs
            .lock()
            .map_err(|err| StructuralError::ProducerExecution(format!("recorder poisoned: {err}")))?
            .push(request.aux_epoch);
        if let Some(err) = self.execute_error.as_ref() {
            return Err(match err {
                StructuralError::ParseTreeProducerUnavailable => {
                    StructuralError::ParseTreeProducerUnavailable
                }
                StructuralError::GenerationNotReady => StructuralError::GenerationNotReady,
                StructuralError::ShardUnavailable => StructuralError::ShardUnavailable,
                StructuralError::LangNotSupported(lang) => {
                    StructuralError::LangNotSupported(lang.clone())
                }
                StructuralError::HoleKindUnsupported(kind) => {
                    StructuralError::HoleKindUnsupported(kind.clone())
                }
                StructuralError::InvalidRequest(message) => {
                    StructuralError::InvalidRequest(message.clone())
                }
                StructuralError::ProducerExecution(message) => {
                    StructuralError::ProducerExecution(message.clone())
                }
                StructuralError::AuxEpochExpired(message) => {
                    StructuralError::AuxEpochExpired(message.clone())
                }
                StructuralError::AuxEpochUnknown(message) => {
                    StructuralError::AuxEpochUnknown(message.clone())
                }
            });
        }
        Ok(self.results.clone())
    }
}

pub(crate) struct PatternRoutingStructuralProducer {
    pub(crate) readiness_calls: AtomicUsize,
    pub(crate) execute_calls: AtomicUsize,
    pub(crate) candidate_scopes: Mutex<Vec<Option<Vec<String>>>>,
}

impl PatternRoutingStructuralProducer {
    pub(crate) fn new() -> Self {
        Self {
            readiness_calls: AtomicUsize::new(0),
            execute_calls: AtomicUsize::new(0),
            candidate_scopes: Mutex::new(Vec::new()),
        }
    }

    pub(crate) fn recorded_scopes(
        &self,
    ) -> Result<Vec<Option<Vec<String>>>, Box<dyn std::error::Error>> {
        let guard = self
            .candidate_scopes
            .lock()
            .map_err(|err| format!("candidate scope state poisoned: {err}"))?;
        Ok(guard.clone())
    }
}

impl StructuralProducerPort for PatternRoutingStructuralProducer {
    fn readiness(&self, _request: &StructuralQueryRequest) -> StructuralReadiness {
        let _prev: usize = self.readiness_calls.fetch_add(1, Ordering::SeqCst);
        StructuralReadiness::Ready
    }

    fn execute(
        &self,
        request: &StructuralQueryRequest,
    ) -> Result<Vec<StructuralMatchCandidate>, StructuralError> {
        let _prev: usize = self.execute_calls.fetch_add(1, Ordering::SeqCst);
        {
            let mut scopes = self.candidate_scopes.lock().map_err(|err| {
                StructuralError::ProducerExecution(format!("candidate scope state poisoned: {err}"))
            })?;
            scopes.push(request.candidate_scope.clone());
        }
        match structural_pattern_key(&request.pattern) {
            Some("alpha") => Ok(vec![
                structural_match_candidate_with_binding("chunk-a", 10, 15, "x", 10, 15),
                structural_match_candidate_with_binding("chunk-shared", 20, 25, "x", 20, 25),
                structural_match_candidate_with_binding("chunk-shared", 5, 10, "x", 5, 10),
            ]),
            Some("beta") => Ok(vec![structural_match_candidate_with_binding(
                "chunk-shared",
                30,
                35,
                "y",
                30,
                35,
            )]),
            Some("gamma") => Ok(vec![structural_match_candidate_with_binding(
                "chunk-a", 40, 45, "z", 40, 45,
            )]),
            Some(other) => Err(StructuralError::InvalidRequest(format!(
                "unexpected structural pattern key `{other}` in test producer"
            ))),
            None => Err(StructuralError::InvalidRequest(
                "missing structural pattern key in test producer".to_string(),
            )),
        }
    }
}

pub(crate) fn structural_pattern_key(
    pattern: &quanta_index_contract::LqStructuralBlock,
) -> Option<&str> {
    match pattern.nodes.first() {
        Some(quanta_index_contract::LqStructuralNode::Literal(text)) => Some(text.trim()),
        _ => None,
    }
}

/// A ready ledger whose pinned generation has a structural authority to
/// pin a read epoch on (QI-BB-020 W2): one chunk, so the route can name
/// the snapshot every leaf reads before it consults the producer.
pub(crate) fn ready_ledger_with_structural_universe() -> Arc<RwLock<Ledger>> {
    let ledger = ready_ledger();
    {
        let mut guard = ledger.write().expect("structural test ledger poisoned");
        install_structural_test_chunk(&mut guard, "chunk-universe", "src/universe.rs", "fn u() {}")
            .expect("structural test chunk install");
    }
    ledger
}

pub(crate) fn structural_dispatcher_with_producer<P>(
    producer: Arc<P>,
) -> Result<SearchPlaneDispatcher, Box<dyn std::error::Error>>
where
    P: StructuralProducerPort + Send + Sync + 'static,
{
    structural_dispatcher_with_producer_and_ledger(
        producer,
        ready_ledger_with_structural_universe(),
    )
}

pub(crate) fn structural_dispatcher_with_producer_and_ledger<P>(
    producer: Arc<P>,
    ledger: Arc<RwLock<Ledger>>,
) -> Result<SearchPlaneDispatcher, Box<dyn std::error::Error>>
where
    P: StructuralProducerPort + Send + Sync + 'static,
{
    Ok(SearchPlaneDispatcher::new(
        Arc::new(RejectLexicalOpener),
        Arc::new(RejectSemanticOpener),
        Arc::new(StubRepoMapQueryPort),
        producer,
        ledger,
        test_activation_catalog()?,
    ))
}

pub(crate) fn structural_dispatcher_mixed<P>(
    producer: Arc<P>,
    lex_opener: Arc<dyn LexicalIndexOpenPort + Send + Sync>,
    ledger: Arc<RwLock<Ledger>>,
) -> Result<SearchPlaneDispatcher, Box<dyn std::error::Error>>
where
    P: StructuralProducerPort + Send + Sync + 'static,
{
    Ok(SearchPlaneDispatcher::new(
        lex_opener,
        Arc::new(RejectSemanticOpener),
        Arc::new(StubRepoMapQueryPort),
        producer,
        ledger,
        test_activation_catalog()?,
    ))
}

pub(crate) fn ready_ledger_with_structural_boolean_chunks() -> Arc<RwLock<Ledger>> {
    let mut ledger = Ledger::default();
    let repo_id =
        RepoId::new("repo-map-ipc").expect("static fixture ID satisfies canonical policy");
    let revision_id =
        RevisionId::new("rev-map-ipc").expect("static fixture ID satisfies canonical policy");
    let generation = ManifestGeneration::new(9);
    ledger.lexical_seal(generation);
    ledger.semantic_seal_with_digest(generation, "manifest-digest-9");
    ledger.record_track_materialized(
        &repo_id,
        &revision_id,
        SearchPlaneTrackKind::Lexical,
        generation,
        None,
    );
    ledger.record_track_seal(&repo_id, &revision_id, SearchPlaneTrackKind::Lexical, generation);
    ledger.record_track_materialized(
        &repo_id,
        &revision_id,
        SearchPlaneTrackKind::Semantic,
        generation,
        Some("manifest-digest-9"),
    );
    ledger.record_track_seal_with_digest(
        &repo_id,
        &revision_id,
        SearchPlaneTrackKind::Semantic,
        generation,
        "manifest-digest-9",
    );
    ledger.record_historically_sealed_search_corpus(
        &repo_id,
        &revision_id,
        generation,
        "manifest-digest-9",
    );
    for (chunk_id, path, text) in [
        ("chunk-a", "src/a.rs", "alpha text"),
        ("chunk-shared", "src/shared.rs", "alpha beta text"),
        ("chunk-beta", "src/b.rs", "beta text"),
    ] {
        install_structural_test_chunk(&mut ledger, chunk_id, path, text)
            .expect("structural test chunk install");
    }
    Arc::new(RwLock::new(ledger))
}

pub(crate) fn install_structural_test_chunk(
    ledger: &mut Ledger,
    chunk_id: &str,
    path: &str,
    text: &str,
) -> TestResult {
    let mut record = structural_test_chunk_record(path, text);
    record.chunk_id = ChunkId::new(chunk_id);
    let op = LexicalChannelOp::UpsertChunk(UpsertChunk {
        repo_id: RepoId::new("repo-map-ipc").expect("static fixture ID satisfies canonical policy"),
        revision_id: RevisionId::new("rev-map-ipc")
            .expect("static fixture ID satisfies canonical policy"),
        generation: ManifestGeneration::new(9),
        chunk_id: ChunkId::new(chunk_id),
        payload: encode_cbor(&record)?,
    });
    ledger.apply_lexical_authority_op(&op, std::time::Instant::now())?;
    Ok(())
}

pub(crate) fn structural_test_chunk_record(path: &str, text: &str) -> ChunkRecord {
    #[expect(
        clippy::manual_unwrap_or,
        clippy::option_if_let_else,
        reason = "Result::unwrap_or is disallowed by clippy.toml; saturate the test text length to u32::MAX"
    )]
    let end_byte = match u32::try_from(text.len()) {
        Ok(len) => len,
        Err(_) => u32::MAX,
    };
    ChunkRecord {
        chunk_id: ChunkId::new("chunk-1"),
        repo_relative_path: RepoRelativePath::new(path),
        language: LanguageCode::from_code_str("rust").expect("rust language code"),
        start_byte: 0,
        end_byte,
        start_line: 1,
        end_line: 1,
        text: text.to_string().into_boxed_str(),
        structural: None,
        parent_chunk_id: None,
        source_repo_id: None,
    }
}

pub(crate) fn structural_state_for_test_chunks(
    chunks: &[(&str, &str, &str)],
) -> Result<crate::readiness::StructuralAuthorityState, Box<dyn std::error::Error>> {
    let ledger = ready_ledger_with_structural_boolean_chunks();
    {
        let mut guard = ledger.write().expect("structural test ledger poisoned");
        for (chunk_id, path, text) in chunks {
            install_structural_test_chunk(&mut guard, chunk_id, path, text)?;
        }
    }
    let guard = ledger.read().expect("structural test ledger poisoned");
    guard
        .structural_state(
            &RepoId::new("repo-map-ipc").expect("static fixture ID satisfies canonical policy"),
            &RevisionId::new("rev-map-ipc").expect("static fixture ID satisfies canonical policy"),
            ManifestGeneration::new(9),
        )
        .cloned()
        .ok_or_else(|| "structural state missing for test chunks".into())
}

pub(crate) fn structural_match_candidate(id: &str) -> StructuralMatchCandidate {
    structural_match_candidate_with_binding(id, 0, 10, "x", 0, 10)
}

pub(crate) fn structural_match_candidate_with_binding(
    id: &str,
    pattern_start_byte: u32,
    pattern_end_byte: u32,
    metavariable: &str,
    start_byte: u32,
    end_byte: u32,
) -> StructuralMatchCandidate {
    StructuralMatchCandidate {
        candidate_id: id.to_string(),
        pattern_start_byte,
        pattern_end_byte,
        bindings: vec![StructuralMatchBinding {
            metavariable: metavariable.to_string(),
            start_byte,
            end_byte,
            start_line: 1,
            end_line: 1,
        }],
    }
}
