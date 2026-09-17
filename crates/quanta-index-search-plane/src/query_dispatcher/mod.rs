//! Search-plane query orchestration using the in-memory readiness ledger as the
//! source of truth.
//!
//! Module map (dependencies point downward only):
//!
//! - `dispatcher` — the `SearchPlaneDispatcher` type, IPC fan-out under the
//!   request budget, per-route metric emission. Depends on `routes`,
//!   `metrics`, `errors`.
//! - `routes/*` — one file per query route (`impl SearchPlaneDispatcher`
//!   blocks). Depend on the support modules below and on `readiness_gate`
//!   / `snapshots` / `planning`; never on each other except
//!   `hybrid`/`hybrid_seed` -> `semantic` (shared embed gate) and
//!   `structural/route` -> its own sub-modules.
//! - `snapshots` — resident opened-generation acquisition via the registries.
//! - `readiness_gate` — ledger snapshot reads shared by routes.
//! - `planning` — the one executable lexical plan; depends on `selection`,
//!   `rev_at_time`.
//! - `semantic_query` — semantic / hybrid selection resolvers, seed fusion,
//!   explanation builders. Depends on `selection`.
//! - support: `selection`, `window`, `keyset_page`, `ranking`, `rev_at_time`,
//!   `text_plane`, `timeref`, `errors`, `metrics`. `rev_at_time` and
//!   `text_plane` depend on `timeref` / `errors`; the rest depend only on
//!   `errors`.

mod dispatcher;
mod errors;
mod keyset_page;
mod metrics;
mod planning;
mod ranking;
mod readiness_gate;
mod rev_at_time;
mod routes;
mod selection;
mod semantic_query;
mod snapshots;
mod text_plane;
mod timeref;
mod window;

pub use dispatcher::{SearchPlaneDispatcher, SearchPlaneQueryDispatcher, SearchPlaneQueryService};
pub use errors::repair_for_code;
pub use selection::make_pin;

#[cfg(test)]
mod tests;
