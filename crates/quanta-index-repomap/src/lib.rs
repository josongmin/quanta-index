#![forbid(unsafe_code)]
#![deny(unused_must_use)]
#![deny(clippy::let_underscore_must_use)]
#![deny(clippy::map_err_ignore)]

pub mod delta;
pub mod materializer;
pub mod model;
pub(crate) mod persistence;
pub mod query;
pub mod reader;
pub mod store;

pub use delta::RepoMapDeltaApplier;
pub use materializer::RepoMapMaterializer;
pub use model::{RepoMapEntryV1, RepoMapSnapshotV1};
pub use query::RepoMapQueryEngine;
pub use reader::RepoMapPinnedReader;
pub use store::RepoMapGenerationStore;
