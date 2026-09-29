//! Shared dispatcher test fixtures: ledgers, catalogs, candidates, obs helpers.

use std::sync::{Arc, RwLock};

use quanta_index_contract::lex::{SymbolKindCode, SymbolKindFamily};
use quanta_index_contract::{
    LQ_VERSION_TAG, LexicalCandidate, LqExpr, LqFilter, LqLeaf, LqOptions, LqQuery, LqSpan,
    ManifestGeneration, RepoId, RepoRelativePath, RevisionId, SearchPlaneQueryIpcResponse,
    SearchPlaneTrackKind, SymbolCandidate,
};
use quanta_index_core::{LexicalIndexOpenPort, SemanticIndexOpenPort};
use tempfile::tempdir;

use crate::observability::{BoundedQueryObsStore, QueryObsSink};
use crate::query_dispatcher::dispatcher::SearchPlaneDispatcher;
use crate::query_dispatcher::selection::make_pin;
use crate::query_dispatcher::tests::support::repo_map::StubRepoMapSnapshotPort;
use crate::query_dispatcher::tests::support::structural::FailClosedStructuralProducer;
use crate::{
    ActivationCatalog, HashingQueryTextEmbedder, Ledger, PreparedSearchCorpusGenerationV1,
    QueryTextEmbedderPort, SEARCH_OWNED_SEMANTIC_DIMENSION, SearchCorpusGenerationV1,
    SnapshotRegistries, SnapshotRegistryPolicy,
};

pub(crate) fn build_probe_query(probe_text: &str) -> LqQuery {
    use quanta_index_contract::LqSpan;
    LqQuery {
        lq_version: LQ_VERSION_TAG,
        expr: LqExpr::Leaf(LqLeaf::Phrase(probe_text.to_string())),
        filters: Vec::new(),
        directives: Vec::new(),
        options: LqOptions::defaults(),
        source_span: LqSpan::eof(u32::try_from(probe_text.len()).map_or(u32::MAX, |n| n)),
    }
}

pub(crate) type TestResult = Result<(), Box<dyn std::error::Error>>;

pub(crate) fn encode_cbor<T: serde::Serialize>(
    value: &T,
) -> Result<Vec<u8>, quanta_index_ipc::IpcError> {
    quanta_index_ipc::encode_cbor_payload(value)
}

pub(crate) fn default_query_embedder() -> Arc<dyn QueryTextEmbedderPort + Send + Sync> {
    Arc::new(HashingQueryTextEmbedder::new(
        SEARCH_OWNED_SEMANTIC_DIMENSION,
    ))
}

pub(crate) fn test_activation_catalog() -> Result<Arc<ActivationCatalog>, Box<dyn std::error::Error>>
{
    let dir = tempdir()?;
    Ok(Arc::new(ActivationCatalog::open(dir.keep())?))
}

pub(crate) fn corpus_generation(
    repo_id: RepoId,
    revision_id: RevisionId,
    manifest_generation: ManifestGeneration,
    manifest_digest: &str,
) -> Result<SearchCorpusGenerationV1, quanta_index_core::CoreError> {
    SearchCorpusGenerationV1::new(
        quanta_index_contract::GenerationSnapshot {
            repo_id: repo_id.clone(),
            revision_id: revision_id.clone(),
            track: SearchPlaneTrackKind::Lexical,
            manifest_generation,
            manifest_digest: manifest_digest.to_string(),
        },
        quanta_index_contract::GenerationSnapshot {
            repo_id,
            revision_id,
            track: SearchPlaneTrackKind::Semantic,
            manifest_generation,
            manifest_digest: manifest_digest.to_string(),
        },
        crate::content_roots_test_support::roots_for_generation(manifest_generation.get()),
    )
}

pub(crate) fn activation_catalog_with_generations(
    generations: &[SearchCorpusGenerationV1],
) -> Result<Arc<ActivationCatalog>, Box<dyn std::error::Error>> {
    let catalog = test_activation_catalog()?;
    for generation in generations {
        let prepared = PreparedSearchCorpusGenerationV1::new(generation.clone(), None)?;
        let activation = catalog.activate_prepared_search_corpus_generation_v1(&prepared)?;
        if activation.active.generation != generation.to_contract_v1() {
            return Err(
                "activation receipt did not preserve the prepared composite generation".into(),
            );
        }
    }
    Ok(catalog)
}

pub(crate) fn ready_ledger() -> Arc<RwLock<Ledger>> {
    let mut ledger = Ledger::default();
    let repo_id =
        RepoId::new("repo-map-ipc").expect("static fixture ID satisfies canonical policy");
    let revision_id =
        RevisionId::new("rev-map-ipc").expect("static fixture ID satisfies canonical policy");
    ledger.lexical_seal(ManifestGeneration::new(9));
    ledger.semantic_seal_with_digest(ManifestGeneration::new(9), "manifest-digest-9");
    ledger.record_track_materialized(
        &repo_id,
        &revision_id,
        SearchPlaneTrackKind::Lexical,
        ManifestGeneration::new(9),
        None,
    );
    ledger.record_track_seal(
        &repo_id,
        &revision_id,
        SearchPlaneTrackKind::Lexical,
        ManifestGeneration::new(9),
    );
    ledger.record_track_materialized(
        &repo_id,
        &revision_id,
        SearchPlaneTrackKind::Semantic,
        ManifestGeneration::new(9),
        Some("manifest-digest-9"),
    );
    ledger.record_track_seal_with_digest(
        &repo_id,
        &revision_id,
        SearchPlaneTrackKind::Semantic,
        ManifestGeneration::new(9),
        "manifest-digest-9",
    );
    ledger.record_historically_sealed_search_corpus(
        &repo_id,
        &revision_id,
        ManifestGeneration::new(9),
        "manifest-digest-9",
    );
    Arc::new(RwLock::new(ledger))
}

pub(crate) fn manual_query(expr: LqExpr, filters: Vec<LqFilter>) -> LqQuery {
    LqQuery {
        lq_version: LQ_VERSION_TAG,
        expr,
        filters,
        directives: Vec::new(),
        options: LqOptions::defaults(),
        source_span: LqSpan::synthetic(0),
    }
}

pub(crate) fn candidate(id: &str, score: f32) -> LexicalCandidate {
    LexicalCandidate {
        source_repo_id: RepoId::new("repo-map-ipc").expect("fixture source repo"),
        source: None,
        preview: None,
        candidate_id: id.to_string(),
        repo_id: RepoId::new("repo-map-ipc").expect("static fixture ID satisfies canonical policy"),
        revision_id: RevisionId::new("rev-map-ipc")
            .expect("static fixture ID satisfies canonical policy"),
        manifest_generation: ManifestGeneration::new(9),
        repo_relative_path: RepoRelativePath::new("src/lib.rs"),
        start_line: 1,
        end_line: 1,
        score,
        snippet: String::new(),
        snippet_hit_offset: None,
        highlights: Vec::new(),
    }
}

pub(crate) fn symbol_candidate(id: &str, score: f32) -> SymbolCandidate {
    SymbolCandidate {
        source_repo_id: RepoId::new("repo-map-ipc").expect("fixture source repo"),
        source: None,
        preview: None,
        candidate_id: id.to_string(),
        repo_id: RepoId::new("repo-map-ipc").expect("static fixture ID satisfies canonical policy"),
        revision_id: RevisionId::new("rev-map-ipc")
            .expect("static fixture ID satisfies canonical policy"),
        manifest_generation: ManifestGeneration::new(9),
        repo_relative_path: RepoRelativePath::new("src/lib.rs"),
        start_line: 1,
        end_line: 1,
        score,
        snippet: "MySymbol crate".to_string(),
        symbol_kind: SymbolKindCode::from_code_str("function")
            .unwrap_or_else(|| std::process::abort()),
        symbol_kind_family: Some(SymbolKindFamily::Callable),
    }
}

pub(crate) fn dispatcher_with_obs(
    lex_opener: Arc<dyn LexicalIndexOpenPort + Send + Sync>,
    sem_opener: Arc<dyn SemanticIndexOpenPort + Send + Sync>,
    obs_sink: Arc<dyn QueryObsSink + Send + Sync>,
) -> Result<SearchPlaneDispatcher, Box<dyn std::error::Error>> {
    Ok(SearchPlaneDispatcher::new_with_obs(
        lex_opener,
        sem_opener,
        SnapshotRegistries::new(SnapshotRegistryPolicy::DEFAULT),
        Arc::new(StubRepoMapSnapshotPort::default()),
        Arc::new(FailClosedStructuralProducer),
        ready_ledger(),
        test_activation_catalog()?,
        default_query_embedder(),
        obs_sink,
    ))
}

pub(crate) fn ready_pin() -> quanta_index_contract::GenerationPin {
    make_pin(
        RepoId::new("repo-map-ipc").expect("static fixture ID satisfies canonical policy"),
        RevisionId::new("rev-map-ipc").expect("static fixture ID satisfies canonical policy"),
        ManifestGeneration::new(9),
    )
}

pub(crate) fn ipc_error_from(
    response: SearchPlaneQueryIpcResponse,
) -> Result<(quanta_index_contract::SearchPlaneErrorCodeV2, String), String> {
    match response {
        SearchPlaneQueryIpcResponse::Error(err) => Ok((err.code, err.message)),
        other @ (SearchPlaneQueryIpcResponse::ActiveGenerationSnapshot(_)
        | SearchPlaneQueryIpcResponse::ResolvedLexicalGeneration(_)
        | SearchPlaneQueryIpcResponse::Text(_)
        | SearchPlaneQueryIpcResponse::Symbol(_)
        | SearchPlaneQueryIpcResponse::Semantic(_)
        | SearchPlaneQueryIpcResponse::SemanticWorkBoundedV1(_)
        | SearchPlaneQueryIpcResponse::Hybrid(_)
        | SearchPlaneQueryIpcResponse::HybridSeed(_)
        | SearchPlaneQueryIpcResponse::History(_)
        | SearchPlaneQueryIpcResponse::Structural(_)
        | SearchPlaneQueryIpcResponse::RepoMapQuery(_)
        | SearchPlaneQueryIpcResponse::Explain(_)
        | SearchPlaneQueryIpcResponse::ClusterMembershipRead(_)
        | SearchPlaneQueryIpcResponse::RuntimeMetadata(_)) => {
            Err(format!("expected Error response, got {other:?}"))
        }
    }
}

pub(crate) fn assert_closed_obs_metrics(
    obs_sink: &Arc<BoundedQueryObsStore>,
    expected: &[&str],
) -> TestResult {
    let names = obs_sink
        .snapshot()
        .into_iter()
        .map(|sample| sample.name.into_string())
        .collect::<Vec<_>>();
    let expected = expected
        .iter()
        .map(|name| (*name).to_string())
        .collect::<Vec<_>>();
    if names != expected {
        return Err(format!("unexpected obs metric names: {names:?}").into());
    }
    let errors = obs_sink.errors();
    if !errors.is_empty() {
        return Err(format!("unexpected obs errors: {errors:?}").into());
    }
    let samples = obs_sink.snapshot();
    for sample in &samples {
        if sample.dimensions.ticket_id.as_ref() != "LXE-10"
            || sample.dimensions.wave_id.as_ref() != "8"
            || sample.dimensions.tenant_id.as_ref() != "local"
            || sample.dimensions.repo_id.as_ref() != "repo-map-ipc"
            || sample.dimensions.generation_id != 9
        {
            return Err(format!("unexpected obs dimensions: {:?}", sample.dimensions).into());
        }
        if sample.name.contains("needle")
            || sample.name.contains("alpha")
            || sample.name.contains("scope")
            || sample.name.contains("fix")
        {
            return Err(format!("metric name leaked query content: {}", sample.name).into());
        }
    }
    Ok(())
}
