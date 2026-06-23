//! Application wiring.

pub mod config;
mod ipc_dispatcher;
pub mod runtime;
pub mod searchd;
pub mod semantic_boot;
pub mod server;

pub use config::{SearchdConfig, SemanticEmbedderProfile};
pub use runtime::SearchdRuntime;
pub use searchd::drive;
pub use server::QueryServer;
