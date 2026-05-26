//! Channel domain — owns validation policy for legacy op streams.
//!
//! This domain does not implement transport. It defines the sequence /
//! generation / seal validation rules that legacy op-stream adapters must
//! preserve when they are exercised in tests or offline tooling.

mod outbound;
mod service;

pub use outbound::ChannelObserver;
pub use service::ChannelDispatchPolicy;
