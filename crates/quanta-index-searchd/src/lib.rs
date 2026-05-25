#![forbid(unsafe_code)]
#![deny(unused_must_use)]
#![deny(clippy::let_underscore_must_use)]
#![deny(clippy::map_err_ignore)]

//! Composition root for the search-plane daemon.

pub mod app;
pub mod cli;

pub use app::{QueryServer, SearchdConfig, SearchdRuntime, drive};
pub use cli::SearchdCommand;
