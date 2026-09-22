//! Search-plane query orchestration using the in-memory readiness ledger as the
//! source of truth.
//!
//! Module map (dependencies point downward only):
//!
//! - `dispatcher` — the `SearchPlaneDispatcher` type, IPC fan-out under the
//!   request budget, per-route metric emission. Depends on `routes`,
//!   `metrics`, `errors`.
//! - `routes/*` — one file per query route (`impl SearchPlaneDispatcher`
//!   blocks). Depend on the support modules below and on `read_view` /
//!   `planning`; never on each other except `hybrid`/`hybrid_seed` ->
//!   `semantic` (shared embed gate) and `structural/route` -> its own
//!   sub-modules.
//! - `read_view` — the one `QueryReadViewV2` a request executes against:
//!   the declared domains acquired once, every ledger read under one
//!   guard, the track handles through its private `snapshots` child.
//!   Routes never reach the ledger or the registries except through it.
//! - `planning` — the one executable lexical plan; depends on
//!   `read_view` (the `rev:at.time(...)` selection view), `selection`,
//!   `rev_at_time`.
//! - `semantic_query` — semantic / hybrid selection resolvers, seed fusion,
//!   explanation builders. Depends on `selection`.
//! - `dense_admission` — the dense lane under exact DSL filters: the
//!   over-fetch / admit / refill loop the `hybrid` and `hybrid_seed` routes
//!   share (QI-BB-018 보완 #3). Depends only on core.
//! - support: `selection`, `window`, `keyset_page`, `ranking`, `rev_at_time`,
//!   `text_plane`, `timeref`, `errors`, `metrics`, `response_budget` (the
//!   ranked lexical page's byte budget). `rev_at_time` and `text_plane`
//!   depend on `timeref` / `errors`; the rest depend only on `errors`.

mod cursor_key;
mod continuation;
mod dense_admission;
mod dispatcher;
mod errors;
mod keyset_page;
mod metrics;
mod planning;
mod ranking;
mod read_view;
mod response_budget;
mod rev_at_time;
mod routes;
mod selection;
mod semantic_query;
mod text_plane;
mod timeref;
mod window;

pub use dispatcher::{SearchPlaneDispatcher, SearchPlaneQueryDispatcher, SearchPlaneQueryService};
pub use errors::repair_for_code;
pub use response_budget::{RESPONSE_ENVELOPE_RESERVE_BYTES, ResponsePayloadBudget};
pub use selection::make_pin;

pub use cursor_key::CursorKeyStore;

#[cfg(test)]
pub(crate) mod tests;
