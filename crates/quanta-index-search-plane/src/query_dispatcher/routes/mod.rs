//! Query routes: one file per IPC request family, each an
//! `impl SearchPlaneDispatcher` block plus its private helpers.

mod cluster_membership;
mod explain;
pub(crate) mod history;
mod history_records;
mod history_relevance;
mod hybrid;
mod hybrid_seed;
mod lexical;
mod repo_map;
pub(crate) mod runtime_metadata;
mod semantic;
pub(crate) mod structural;
