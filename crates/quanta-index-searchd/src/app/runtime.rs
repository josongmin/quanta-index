//! Composed runtime artefacts. Built once per daemon process.

use std::sync::{Arc, RwLock};

use anyhow::Result;
use quanta_index_channel::{open_lexical_subscriber, open_semantic_subscriber};
use quanta_index_core::{
    LexicalIndexBuildPort, LexicalIndexOpenPort, SemanticIndexBuildPort, SemanticIndexOpenPort,
};
use quanta_index_lexical::LexicalAdapter;
use quanta_index_semantic::SemanticAdapter;

use crate::app::config::SearchdConfig;
use crate::app::dispatcher::ChannelDispatcher;
use crate::app::query::SearchPlaneDispatcher;
use crate::app::server::QueryServer;
use crate::runtime::Ledger;

/// Composed runtime: subscribers, adapters, ledger, dispatcher, query server.
pub struct SearchdRuntime {
    pub config: SearchdConfig,
    pub dispatcher: ChannelDispatcher,
    pub query_server: QueryServer,
}

impl SearchdRuntime {
    /// Build all components from the resolved config.
    pub fn build(config: SearchdConfig) -> Result<Self> {
        let state_root = config.state_root().to_path_buf();
        let lex_sub = open_lexical_subscriber(&state_root)?;
        let sem_sub = open_semantic_subscriber(&state_root)?;

        let lex_adapter: Arc<LexicalAdapter> = Arc::new(LexicalAdapter::with_state_root(
            state_root.join("indexes/lexical"),
        ));
        let sem_adapter: Arc<SemanticAdapter> = Arc::new(SemanticAdapter::new());
        let ledger = Arc::new(RwLock::new(Ledger::new()));

        // Upcast the concrete adapters to their domain port trait objects. The
        // composition root is the only place that names the concrete adapter
        // types; downstream collaborators depend on the ports only.
        let lex_build_port: Arc<dyn LexicalIndexBuildPort + Send + Sync> =
            Arc::clone(&lex_adapter) as _;
        let lex_open_port: Arc<dyn LexicalIndexOpenPort + Send + Sync> =
            Arc::clone(&lex_adapter) as _;
        let sem_build_port: Arc<dyn SemanticIndexBuildPort + Send + Sync> =
            Arc::clone(&sem_adapter) as _;
        let sem_open_port: Arc<dyn SemanticIndexOpenPort + Send + Sync> =
            Arc::clone(&sem_adapter) as _;

        let dispatcher = ChannelDispatcher::new(
            lex_sub,
            sem_sub,
            lex_build_port,
            sem_build_port,
            Arc::clone(&ledger),
        );

        let plane_dispatcher = Arc::new(SearchPlaneDispatcher::new(
            lex_open_port,
            sem_open_port,
            Arc::clone(&ledger),
        ));
        let query_server = QueryServer::bind(config.socket_path(), plane_dispatcher)
            .map_err(anyhow::Error::from)?;

        Ok(Self {
            config,
            dispatcher,
            query_server,
        })
    }
}
