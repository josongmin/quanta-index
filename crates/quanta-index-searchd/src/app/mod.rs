//! Application wiring.

pub mod config;
mod ipc_dispatcher;
pub mod runtime;
pub mod searchd;
pub mod server;

pub use config::SearchdConfig;
pub use runtime::SearchdRuntime;
pub use searchd::drive;
pub use server::QueryServer;
