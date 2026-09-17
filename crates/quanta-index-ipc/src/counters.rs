//! What one socket server counts while it serves (QI-BB-015).
//!
//! The accept loop and every connection thread bump these without a lock;
//! a scrape reads them the same way. The counters are handed to the
//! server at bind so the composition root can register them with the
//! metrics scrape before any connection exists.

use std::sync::atomic::{AtomicU64, Ordering};

use quanta_index_core::{CoreError, MetricPointV1, MetricSourcePort};

/// The counts one server keeps, all monotonic but `connections_live`.
#[derive(Debug)]
pub struct IpcServerCounters {
    /// The metric-name segment for this server (`query`, `control`,
    /// `ingest`).
    plane: &'static str,
    connections_accepted: AtomicU64,
    /// Connections closed at accept because the policy's cap was reached.
    connections_refused: AtomicU64,
    connections_live: AtomicU64,
    /// Requests that could not be decoded; the connection closed.
    request_decode_failures: AtomicU64,
    /// Requests that found no dispatch slot within the queue wait.
    requests_overloaded: AtomicU64,
    /// Requests that arrived after shutdown began; not dispatched.
    requests_refused_shutting_down: AtomicU64,
    /// Requests the dispatcher answered.
    requests_dispatched: AtomicU64,
    /// Dispatched requests whose peer was gone when the answer was ready.
    peer_hangups: AtomicU64,
}

/// One consistent-enough read of [`IpcServerCounters`]: each field is
/// atomic, the set is not.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct IpcServerCountersSnapshot {
    pub connections_accepted: u64,
    pub connections_refused: u64,
    pub connections_live: u64,
    pub request_decode_failures: u64,
    pub requests_overloaded: u64,
    pub requests_refused_shutting_down: u64,
    pub requests_dispatched: u64,
    pub peer_hangups: u64,
}

impl IpcServerCounters {
    /// Counters for the server of one plane.
    ///
    /// `plane` becomes the second segment of every metric name
    /// (`ipc_<plane>_…`), so it must be `[a-z][a-z0-9_]*`; a scrape refuses
    /// a source whose names are not.
    #[must_use]
    pub const fn for_plane(plane: &'static str) -> Self {
        Self {
            plane,
            connections_accepted: AtomicU64::new(0),
            connections_refused: AtomicU64::new(0),
            connections_live: AtomicU64::new(0),
            request_decode_failures: AtomicU64::new(0),
            requests_overloaded: AtomicU64::new(0),
            requests_refused_shutting_down: AtomicU64::new(0),
            requests_dispatched: AtomicU64::new(0),
            peer_hangups: AtomicU64::new(0),
        }
    }

    #[must_use]
    pub const fn plane(&self) -> &'static str {
        self.plane
    }

    #[must_use]
    pub fn snapshot(&self) -> IpcServerCountersSnapshot {
        IpcServerCountersSnapshot {
            connections_accepted: self.connections_accepted.load(Ordering::Acquire),
            connections_refused: self.connections_refused.load(Ordering::Acquire),
            connections_live: self.connections_live.load(Ordering::Acquire),
            request_decode_failures: self.request_decode_failures.load(Ordering::Acquire),
            requests_overloaded: self.requests_overloaded.load(Ordering::Acquire),
            requests_refused_shutting_down: self
                .requests_refused_shutting_down
                .load(Ordering::Acquire),
            requests_dispatched: self.requests_dispatched.load(Ordering::Acquire),
            peer_hangups: self.peer_hangups.load(Ordering::Acquire),
        }
    }

    pub(crate) fn connection_accepted(&self) {
        let _prior = self.connections_accepted.fetch_add(1, Ordering::AcqRel);
        let _prior = self.connections_live.fetch_add(1, Ordering::AcqRel);
    }

    pub(crate) fn connection_refused(&self) {
        let _prior = self.connections_refused.fetch_add(1, Ordering::AcqRel);
    }

    pub(crate) fn connection_closed(&self) {
        let _prior = self.connections_live.fetch_sub(1, Ordering::AcqRel);
    }

    pub(crate) fn connections_live(&self) -> u64 {
        self.connections_live.load(Ordering::Acquire)
    }

    pub(crate) fn request_decode_failed(&self) {
        let _prior = self.request_decode_failures.fetch_add(1, Ordering::AcqRel);
    }

    pub(crate) fn request_overloaded(&self) {
        let _prior = self.requests_overloaded.fetch_add(1, Ordering::AcqRel);
    }

    pub(crate) fn request_refused_shutting_down(&self) {
        let _prior = self
            .requests_refused_shutting_down
            .fetch_add(1, Ordering::AcqRel);
    }

    pub(crate) fn request_dispatched(&self, peer_hung_up: bool) {
        let _prior = self.requests_dispatched.fetch_add(1, Ordering::AcqRel);
        if peer_hung_up {
            let _prior = self.peer_hangups.fetch_add(1, Ordering::AcqRel);
        }
    }

    fn metric_name(&self, suffix: &str) -> String {
        format!("ipc_{}_{suffix}", self.plane)
    }
}

impl MetricSourcePort for IpcServerCounters {
    fn scrape(&self) -> Result<Vec<MetricPointV1>, CoreError> {
        let snapshot = self.snapshot();
        Ok(vec![
            MetricPointV1::counter(
                self.metric_name("connections_accepted_total"),
                snapshot.connections_accepted,
            ),
            MetricPointV1::counter(
                self.metric_name("connections_refused_total"),
                snapshot.connections_refused,
            ),
            MetricPointV1::gauge_count(
                self.metric_name("connections_live"),
                snapshot.connections_live,
            ),
            MetricPointV1::counter(
                self.metric_name("request_decode_failures_total"),
                snapshot.request_decode_failures,
            ),
            MetricPointV1::counter(
                self.metric_name("requests_overloaded_total"),
                snapshot.requests_overloaded,
            ),
            MetricPointV1::counter(
                self.metric_name("requests_refused_shutting_down_total"),
                snapshot.requests_refused_shutting_down,
            ),
            MetricPointV1::counter(
                self.metric_name("requests_dispatched_total"),
                snapshot.requests_dispatched,
            ),
            MetricPointV1::counter(
                self.metric_name("peer_hangups_total"),
                snapshot.peer_hangups,
            ),
        ])
    }
}
