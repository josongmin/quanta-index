//! One `QueryReadViewV2` per request (plan §5.6 / §7.1).
//!
//! - `view` — the request, the view, its acquisition and the trace
//!   attachment.
//! - `snapshots` — resident opened-generation acquisition via the
//!   registries, with the hit / coalesced / cold-open metrics; private to
//!   this module, so a route can reach a track handle only through the
//!   view.

mod snapshots;
mod view;

pub(crate) use view::{AuxEpochPinsV1, QueryReadViewV2, ReadViewRequestV1, attach_read_view_trace};

#[cfg(test)]
pub(crate) use view::assemble_for_test;
