#![forbid(unsafe_code)]
#![deny(unused_must_use)]
#![deny(clippy::let_underscore_must_use)]
#![deny(clippy::map_err_ignore)]
#![expect(
    clippy::multiple_crate_versions,
    reason = "transitive duplicates from tantivy + tokio; tracked by cargo-deny skip-tree"
)]

//! Concrete runtime wiring for the search-plane daemon.

use std::sync::Arc;
use std::sync::atomic::AtomicBool;

use anyhow::Result;
use quanta_index_channel::{
    BundleChannelPublisher, open_lexical_publisher, open_lexical_subscriber,
    open_semantic_publisher, open_semantic_subscriber,
};
use quanta_index_core::{
    LexicalIndexBuildPort, LexicalIndexOpenPort, LexicalIngestPort, RepoMapBundleIngestPort,
    RepoMapGenerationActivatePort, RepoMapQueryPort, SemanticIndexBuildPort, SemanticIndexOpenPort,
    SemanticIngestPort,
};
use quanta_index_lexical::LexicalAdapter;
use quanta_index_repomap::RepoMapGenerationStore;
use quanta_index_search_plane::{
    ActivationCatalog, ChannelLexicalIngestAdapter, ChannelSemanticIngestAdapter,
};
use quanta_index_searchd::app::runtime::SearchdRuntimeParts;
use quanta_index_searchd::{SearchdCommand, SearchdConfig, SearchdRuntime, drive};
use quanta_index_semantic::SemanticAdapter;

pub fn build_runtime(config: SearchdConfig) -> Result<SearchdRuntime> {
    let state_root = config.state_root().to_path_buf();
    let lex_sub = open_lexical_subscriber(&state_root)?;
    let sem_sub = open_semantic_subscriber(&state_root)?;
    let lex_publisher: Arc<
        dyn BundleChannelPublisher<Op = quanta_index_contract::LexicalChannelOp> + Send + Sync,
    > = Arc::new(open_lexical_publisher(&state_root)?);
    let sem_publisher: Arc<
        dyn BundleChannelPublisher<Op = quanta_index_contract::SemanticChannelOp> + Send + Sync,
    > = Arc::new(open_semantic_publisher(&state_root)?);

    let lex_adapter: Arc<LexicalAdapter> = Arc::new(LexicalAdapter::with_state_root(
        state_root.join("indexes/lexical"),
    ));
    let sem_adapter: Arc<SemanticAdapter> = Arc::new(SemanticAdapter::new());
    let repo_map_store = Arc::new(
        RepoMapGenerationStore::with_persistence_root(state_root.join("repo-map"))
            .map_err(anyhow::Error::from)?,
    );
    let activation_catalog = Arc::new(ActivationCatalog::open(state_root.join("activations"))?);

    let lex_build_port: Arc<dyn LexicalIndexBuildPort + Send + Sync> = lex_adapter.clone();
    let lex_open_port: Arc<dyn LexicalIndexOpenPort + Send + Sync> = lex_adapter;
    let lex_ingest_port: Arc<dyn LexicalIngestPort + Send + Sync> =
        Arc::new(ChannelLexicalIngestAdapter::new(lex_publisher));
    let sem_build_port: Arc<dyn SemanticIndexBuildPort + Send + Sync> = sem_adapter.clone();
    let sem_open_port: Arc<dyn SemanticIndexOpenPort + Send + Sync> = sem_adapter;
    let sem_ingest_port: Arc<dyn SemanticIngestPort + Send + Sync> =
        Arc::new(ChannelSemanticIngestAdapter::new(sem_publisher));
    let repo_map_query_port: Arc<dyn RepoMapQueryPort + Send + Sync> = repo_map_store.clone();
    let repo_map_bundle_ingest_port: Arc<dyn RepoMapBundleIngestPort + Send + Sync> =
        repo_map_store.clone();
    let repo_map_generation_activate_port: Arc<dyn RepoMapGenerationActivatePort + Send + Sync> =
        repo_map_store;

    SearchdRuntime::assemble(
        config,
        SearchdRuntimeParts {
            lex_sub,
            sem_sub,
            lex_build_port,
            lex_open_port,
            lex_ingest_port,
            sem_build_port,
            sem_open_port,
            sem_ingest_port,
            repo_map_query_port,
            repo_map_bundle_ingest_port,
            repo_map_generation_activate_port,
            activation_catalog,
        },
    )
}

pub fn run(command: SearchdCommand) -> Result<()> {
    let config = command.into_config()?;
    let runtime = build_runtime(config)?;
    let shutdown = Arc::new(AtomicBool::new(false));
    drive(runtime, shutdown)
}
