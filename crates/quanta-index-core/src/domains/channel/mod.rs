//! Channel domain — owns the per-track event stream policy for searchd.
//!
//! This domain does NOT implement the WAL transport (that's in
//! `quanta-index-channel::backends`). It defines the policy that
//! `searchd::app::dispatcher` enforces on every event observed from a
//! subscriber: sequence monotonicity, generation-scope validation, and seal
//! ordering.

mod outbound;
mod service;

pub use outbound::ChannelObserver;
pub use service::ChannelDispatchPolicy;
