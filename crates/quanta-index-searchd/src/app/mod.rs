//! Application wiring.

pub mod config;
pub mod dispatcher;
pub mod query;
pub mod runtime;
pub mod searchd;
pub mod server;

pub use config::SearchdConfig;
pub use dispatcher::ChannelDispatcher;
pub use query::SearchPlaneDispatcher;
pub use runtime::SearchdRuntime;
pub use searchd::run;
pub use server::QueryServer;
