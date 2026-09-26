#![forbid(unsafe_code)]

//! OBS-01 — Observability + SLO instrumentation typed surface.
//!
//! This crate ships the **typed contract for emission** of the LQ
//! observability surface under
//! `docs/adr/JUN-02-001-search-dsl-authority-and-runtime-contract.md`.
//! It does NOT ship a transport (no `OTel` wire, no Prometheus exporter)
//! — that lives in the integration ticket which depends on this crate.
//!
//! ## Surface choice
//!
//! ADR-009 (OBS-01 § 4.6): OpenTelemetry SDK + Prometheus exporter in
//! sidecar mode. The wire integration wraps this crate's typed shapes;
//! this crate carries the shapes themselves.
//!
//! ## Hard locks
//!
//! - 14 child span kinds under [`span::ROOT_SPAN`] (`lq.query`), each
//!   carrying `error_code` + `budget_remaining_ms` attributes.
//! - Metric schema with the closed dimension set
//!   `{ticket_id, wave_id, tenant_id, repo_id, generation_id}` per
//!   [`dim::Dimensions`].
//! - Cardinality guard 4-layer defense — see
//!   [`cardinality_guard::CardinalityGuard`].
//! - Audit-log fields per [`audit::AuditEntry`]; outcome is
//!   `granted` / `denied` / `error`.
//! - D18 — every wire shape is hand-rolled serde. No proc-macro derives.
//! - No silent failure, no silent fallback. Every overflow surfaces a
//!   typed [`errors::ObsError`] with
//!   [`errors::ObsErrorCode::ObsCardinalityGuard`].

pub mod audit;
pub mod cardinality_guard;
pub mod dim;
pub mod errors;
pub mod metric;
pub mod slo;
pub mod span;

pub use audit::{AuditEntry, AuditOutcome, validate_audit};
pub use cardinality_guard::{CardinalityGuard, OBS_OVERFLOW_LABEL};
pub use dim::{
    Dimensions, MAX_DISTINCT_REPOS_PER_TENANT, MAX_DISTINCT_TENANTS_GLOBAL, PER_FIELD_CHAR_CAP,
    validate_dimensions,
};
pub use errors::{ObsError, ObsErrorCode};
pub use metric::{MetricKind, MetricSample};
pub use slo::{
    BRIDGE_ROUTE_SLO, BRIDGE_TRANSLATE_SLO, HISTORY_SINGLE_REPO_SLO, HYBRID_MERGE_SLO,
    LEX_100_REPO_FANOUT_SLO, LEX_SINGLE_REPO_SLO, LEX_SYMBOL_SINGLE_REPO_SLO, RUNTIME_CATALOG_SLO,
    SEMANTIC_ANN_SLO, STRUCTURAL_SINGLE_REPO_SLO, SloTarget, SloViolation,
};
pub use span::{ROOT_SPAN, SpanEvent, SpanKind};
