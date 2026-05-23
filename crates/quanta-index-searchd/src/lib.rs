#![forbid(unsafe_code)]
#![deny(unused_must_use)]
#![deny(clippy::let_underscore_must_use)]
#![deny(clippy::map_err_ignore)]

pub mod app;
pub mod cli;
pub mod query;
pub mod runtime;

pub use app::run;
