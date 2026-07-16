#![forbid(unsafe_code)]
#![deny(unused_must_use)]
#![deny(clippy::let_underscore_must_use)]
#![deny(clippy::map_err_ignore)]
#![expect(
    clippy::multiple_crate_versions,
    reason = "transitive duplicates from tantivy + tokio + lancedb (arrow/datafusion subtree); each is named-scoped in deny.toml via skip-tree"
)]

//! Concrete runtime wiring for the search-plane daemon.

use std::sync::Arc;
use std::sync::atomic::AtomicBool;

use anyhow::Result;
use quanta_index_core::{
    FileContributorIngestPort, FileOwnershipIngestPort, GenerationIdentityValidatePort,
    IncompleteGenerationDiscardPort, LexicalIndexOpenPort, RepoCommitRecencyIngestPort,
    RepoDescriptionIngestPort, RepoMapBundleIngestPort, RepoMapGenerationActivatePort,
    RepoMapQueryPort, RepoMetaIngestPort, RepoTopicIngestPort, SealedGenerationScanPort,
    SearchCorpusBatchBuildPort, SemanticBatchBuildPort, SemanticIndexOpenPort,
};
use quanta_index_lexical::LexicalAdapter;
use quanta_index_repomap::RepoMapGenerationStore;
use quanta_index_search_plane::{
    ActivationCatalog, AuxiliaryAuthorityStore, LegacySemanticJournalStore,
};
use quanta_index_searchd::app::runtime::{SearchdRuntimeParts, StateRootLease};
use quanta_index_searchd::{SearchdCommand, SearchdConfig, SearchdRuntime, drive};
use quanta_index_semantic::SemanticAdapter;

pub fn build_runtime(config: SearchdConfig) -> Result<SearchdRuntime> {
    let search_corpus_history_retention = config.search_corpus_history_retention_policy()?;
    let state_root = config.state_root().to_path_buf();
    // Acquire process ownership before any adapter or authority store opens the
    // shared root. No loser may observe or mutate partially initialized state.
    let state_root_lease = StateRootLease::acquire(&state_root)?;
    let lex_adapter: Arc<LexicalAdapter> = Arc::new(LexicalAdapter::with_state_root(
        state_root.join("indexes/lexical"),
    ));
    let sem_adapter: Arc<SemanticAdapter> = Arc::new(SemanticAdapter::with_state_root(
        quanta_index_semantic::semantic_state_root(&state_root),
    )?);
    let repo_map_store = Arc::new(
        RepoMapGenerationStore::with_persistence_root(state_root.join("repo-map"))
            .map_err(anyhow::Error::from)?,
    );
    let activation_catalog = Arc::new(ActivationCatalog::open(state_root.join("activations"))?);
    let aux_authority_store = Arc::new(AuxiliaryAuthorityStore::open(
        state_root.join("authorities"),
        search_corpus_history_retention,
    )?);
    let legacy_semantic_journal_store = Arc::new(LegacySemanticJournalStore::open(
        state_root.join("semantic"),
    )?);

    let search_corpus_build_port: Arc<dyn SearchCorpusBatchBuildPort + Send + Sync> =
        lex_adapter.clone();
    let lexical_generation_scanner: Arc<dyn SealedGenerationScanPort + Send + Sync> =
        lex_adapter.clone();
    let lexical_generation_validator: Arc<dyn GenerationIdentityValidatePort + Send + Sync> =
        lex_adapter.clone();
    let lexical_incomplete_discard: Arc<dyn IncompleteGenerationDiscardPort + Send + Sync> =
        lex_adapter.clone();
    let lex_open_port: Arc<dyn LexicalIndexOpenPort + Send + Sync> = lex_adapter.clone();
    let repo_commit_recency_ingest_port: Arc<dyn RepoCommitRecencyIngestPort + Send + Sync> =
        lex_adapter.clone();
    let repo_topic_ingest_port: Arc<dyn RepoTopicIngestPort + Send + Sync> = lex_adapter.clone();
    let repo_description_ingest_port: Arc<dyn RepoDescriptionIngestPort + Send + Sync> =
        lex_adapter.clone();
    let file_ownership_ingest_port: Arc<dyn FileOwnershipIngestPort + Send + Sync> =
        lex_adapter.clone();
    let file_contributor_ingest_port: Arc<dyn FileContributorIngestPort + Send + Sync> =
        lex_adapter.clone();
    let repo_meta_ingest_port: Arc<dyn RepoMetaIngestPort + Send + Sync> = lex_adapter;
    let sem_build_port: Arc<dyn SemanticBatchBuildPort + Send + Sync> = sem_adapter.clone();
    let semantic_generation_validator: Arc<dyn GenerationIdentityValidatePort + Send + Sync> =
        sem_adapter.clone();
    let semantic_incomplete_discard: Arc<dyn IncompleteGenerationDiscardPort + Send + Sync> =
        sem_adapter.clone();
    let sem_open_port: Arc<dyn SemanticIndexOpenPort + Send + Sync> = sem_adapter;
    let repo_map_query_port: Arc<dyn RepoMapQueryPort + Send + Sync> = repo_map_store.clone();
    let repo_map_bundle_ingest_port: Arc<dyn RepoMapBundleIngestPort + Send + Sync> =
        repo_map_store.clone();
    let repo_map_generation_activate_port: Arc<dyn RepoMapGenerationActivatePort + Send + Sync> =
        repo_map_store;

    SearchdRuntime::assemble(
        config,
        SearchdRuntimeParts {
            state_root_lease,
            search_corpus_build_port,
            lexical_generation_scanner,
            lexical_generation_validator,
            lexical_incomplete_discard,
            lex_open_port,
            repo_commit_recency_ingest_port,
            repo_topic_ingest_port,
            repo_description_ingest_port,
            file_ownership_ingest_port,
            file_contributor_ingest_port,
            repo_meta_ingest_port,
            sem_build_port,
            semantic_generation_validator,
            semantic_incomplete_discard,
            sem_open_port,
            repo_map_query_port,
            repo_map_bundle_ingest_port,
            repo_map_generation_activate_port,
            activation_catalog,
            aux_authority_store,
            legacy_semantic_journal_store,
        },
    )
}

pub fn run(command: SearchdCommand) -> Result<()> {
    let config = command.into_config()?;
    let runtime = build_runtime(config)?;
    let shutdown = Arc::new(AtomicBool::new(false));
    drive(runtime, &shutdown)
}
