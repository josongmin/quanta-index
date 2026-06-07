//! Relevance / ranking-quality rail (J7Q-01A).
//!
//! Construct-by-seed judged corpus + pure ranking metrics + per-route-family
//! report. The single source of truth for "what is relevant" is the checked-in
//! [`corpus`] manifest, not a human re-judging each run. The external
//! Sourcegraph lexical overlap floor (J7Q-01B) is a separate side lane; until a
//! local Sourcegraph instance is provisioned, the overlap artifact is emitted as
//! `unprovisioned`, never as a silent pass.

pub mod corpus;
pub mod metrics;
pub mod report;
