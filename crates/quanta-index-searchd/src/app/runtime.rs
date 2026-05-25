//! Composed runtime artefacts. Built once per daemon process.

use std::sync::{Arc, RwLock};

use anyhow::Result;
use quanta_index_channel::{LexicalWalSubscriber, SemanticWalSubscriber};
use quanta_index_contract::{
    SearchPlaneControlIpcRequest, SearchPlaneControlIpcResponse, SearchPlaneIngestIpcRequest,
    SearchPlaneIngestIpcResponse, SearchPlaneQueryIpcRequest, SearchPlaneQueryIpcResponse,
};
use quanta_index_core::{
    LexicalIndexBuildPort, LexicalIndexOpenPort, LexicalIngestPort, RepoMapBundleIngestPort,
    RepoMapGenerationActivatePort, RepoMapQueryPort, SemanticIndexBuildPort, SemanticIndexOpenPort,
    SemanticIngestPort,
};
use quanta_index_ipc::IpcDispatcher;
use quanta_index_search_plane::{
    ActivationCatalog, ChannelDispatcher, HistoryIngestPort, Ledger, RuntimeMetadataIngestPort,
    SearchPlaneControlDispatcher, SearchPlaneDispatcher, SearchPlaneIngestDispatcher,
    StructuralIngestPort,
};

use crate::app::config::SearchdConfig;
use crate::app::ipc_dispatcher::{
    SearchPlaneControlIpcAdapter, SearchPlaneIngestIpcAdapter, SearchPlaneQueryIpcAdapter,
};
use crate::app::server::{
    SearchPlaneControlServer, SearchPlaneIngestServer, SearchPlaneQueryServer,
};

pub struct SearchdRuntimeParts {
    pub lex_sub: LexicalWalSubscriber,
    pub sem_sub: SemanticWalSubscriber,
    pub lex_build_port: Arc<dyn LexicalIndexBuildPort + Send + Sync>,
    pub lex_open_port: Arc<dyn LexicalIndexOpenPort + Send + Sync>,
    pub sem_build_port: Arc<dyn SemanticIndexBuildPort + Send + Sync>,
    pub sem_open_port: Arc<dyn SemanticIndexOpenPort + Send + Sync>,
    pub repo_map_query_port: Arc<dyn RepoMapQueryPort + Send + Sync>,
    pub repo_map_bundle_ingest_port: Arc<dyn RepoMapBundleIngestPort + Send + Sync>,
    pub repo_map_generation_activate_port: Arc<dyn RepoMapGenerationActivatePort + Send + Sync>,
    /// QI-RT-01: typed lexical ingest path. Concrete adapter (channel-backed)
    /// is wired here by the composition root; downstream code only sees the
    /// `dyn LexicalIngestPort` shape.
    pub lex_ingest_port: Arc<dyn LexicalIngestPort + Send + Sync>,
    /// QI-RT-01: typed semantic ingest path.
    pub sem_ingest_port: Arc<dyn SemanticIngestPort + Send + Sync>,
    /// SDK-first source-authority history ingest.
    pub history_ingest_port: Arc<dyn HistoryIngestPort + Send + Sync>,
    /// SDK-first runtime dirty ingest.
    pub runtime_ingest_port: Arc<dyn RuntimeMetadataIngestPort + Send + Sync>,
    /// SDK-first structural parse-tree ingest.
    pub structural_ingest_port: Arc<dyn StructuralIngestPort + Send + Sync>,
    pub activation_catalog: Arc<ActivationCatalog>,
}

/// Composed runtime: subscribers, adapters, ledger, dispatcher, query server.
pub struct SearchdRuntime {
    pub config: SearchdConfig,
    pub dispatcher: ChannelDispatcher,
    pub query_server: SearchPlaneQueryServer<
        dyn IpcDispatcher<SearchPlaneQueryIpcRequest, SearchPlaneQueryIpcResponse>,
    >,
    pub control_server: SearchPlaneControlServer<
        dyn IpcDispatcher<SearchPlaneControlIpcRequest, SearchPlaneControlIpcResponse>,
    >,
    pub ingest_server: SearchPlaneIngestServer<
        dyn IpcDispatcher<SearchPlaneIngestIpcRequest, SearchPlaneIngestIpcResponse>,
    >,
    pub repo_map_query_port: Arc<dyn RepoMapQueryPort + Send + Sync>,
}

impl SearchdRuntime {
    /// Assemble the runtime from externally-supplied subscribers and ports.
    pub fn assemble(config: SearchdConfig, parts: SearchdRuntimeParts) -> Result<Self> {
        let SearchdRuntimeParts {
            lex_sub,
            sem_sub,
            lex_build_port,
            lex_open_port,
            sem_build_port,
            sem_open_port,
            repo_map_query_port,
            repo_map_bundle_ingest_port,
            repo_map_generation_activate_port,
            lex_ingest_port,
            sem_ingest_port,
            history_ingest_port,
            runtime_ingest_port,
            structural_ingest_port,
            activation_catalog,
        } = parts;
        let ledger = Arc::new(RwLock::new(Ledger::new()));
        let dispatcher = ChannelDispatcher::new(
            lex_sub,
            sem_sub,
            lex_build_port,
            sem_build_port,
            Arc::clone(&ledger),
        );

        let query_dispatcher = Arc::new(SearchPlaneDispatcher::new(
            lex_open_port,
            sem_open_port,
            Arc::clone(&repo_map_query_port),
            Arc::clone(&ledger),
            activation_catalog.clone(),
        ));
        let control_dispatcher = Arc::new(SearchPlaneControlDispatcher::new(
            repo_map_generation_activate_port,
            activation_catalog,
        ));
        let ingest_dispatcher = Arc::new(SearchPlaneIngestDispatcher::new(
            lex_ingest_port,
            sem_ingest_port,
            history_ingest_port,
            runtime_ingest_port,
            structural_ingest_port,
            repo_map_bundle_ingest_port,
        ));
        let query_adapter: Arc<
            dyn IpcDispatcher<SearchPlaneQueryIpcRequest, SearchPlaneQueryIpcResponse>,
        > = Arc::new(SearchPlaneQueryIpcAdapter::new(query_dispatcher));
        let query_server = SearchPlaneQueryServer::bind(
            "quanta-index-query-uds",
            config.query_socket_path(),
            query_adapter,
        )
        .map_err(anyhow::Error::from)?;
        let control_adapter: Arc<
            dyn IpcDispatcher<SearchPlaneControlIpcRequest, SearchPlaneControlIpcResponse>,
        > = Arc::new(SearchPlaneControlIpcAdapter::new(control_dispatcher));
        let control_server = SearchPlaneControlServer::bind(
            "quanta-index-control-uds",
            config.control_socket_path(),
            control_adapter,
        )
        .map_err(anyhow::Error::from)?;
        let ingest_adapter: Arc<
            dyn IpcDispatcher<SearchPlaneIngestIpcRequest, SearchPlaneIngestIpcResponse>,
        > = Arc::new(SearchPlaneIngestIpcAdapter::new(ingest_dispatcher));
        let ingest_server = SearchPlaneIngestServer::bind(
            "quanta-index-ingest-uds",
            config.ingest_socket_path(),
            ingest_adapter,
        )
        .map_err(anyhow::Error::from)?;

        Ok(Self {
            config,
            dispatcher,
            query_server,
            control_server,
            ingest_server,
            repo_map_query_port,
        })
    }
}
