#![forbid(unsafe_code)]
#![deny(unused_must_use)]
#![deny(clippy::let_underscore_must_use)]
#![deny(clippy::map_err_ignore)]
// Composition-root crate transitively depends on Tantivy + Lance + RusQLite +
// Tokio. Their dep graphs unavoidably duplicate utility crates (hashbrown,
// rand, rustix, windows-sys, etc.). cargo-deny's [bans] skip-tree owns the
// supply-chain verdict; this `expect` silences clippy's cargo group which
// reports the same condition orthogonally.
#![expect(
    clippy::multiple_crate_versions,
    reason = "transitive duplicates from lance + tantivy + rusqlite + tokio; tracked by cargo-deny skip-tree"
)]

pub mod app;
pub mod cli;
pub mod query;
pub mod runtime;

pub use app::run;

// Composition-root adapter crates (wired in Phase 2+; keep deps until app uses them).
use quanta_index_ipc as _;
use quanta_index_lexical as _;
use quanta_index_semantic as _;
