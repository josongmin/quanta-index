//! What one socket server counts while it serves (QI-BB-015).
//!
//! The accept loop and every connection thread bump these without a lock;
//! a scrape reads them the same way. The counters are handed to the
//! server at bind so the composition root can register them with the
//! metrics scrape before any connection exists.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use quanta_index_core::{CoreError, MetricPointV1, MetricSourcePort};

/// The counts one server keeps, all monotonic but `connections_live` and
/// `dispatch_in_flight`.
#[derive(Debug)]
pub struct IpcServerCounters {
    /// The metric-name segment for this server (`query`, `control`,
    /// `ingest`).
    plane: &'static str,
    connections_accepted: AtomicU64,
    /// Connections closed at accept because the policy's cap was reached.
    connections_refused: AtomicU64,
    /// Connections closed at accept because the peer's credentials are not
    /// admitted by the socket's access policy (QI-BB-014); no frame was
    /// read.
    peers_refused: AtomicU64,
    /// Connections closed at accept because the kernel did not report the
    /// peer's credentials; refused, never admitted.
    peer_credentials_unreadable: AtomicU64,
    connections_live: AtomicU64,
    /// Requests that could not be decoded; the connection closed.
    request_decode_failures: AtomicU64,
    /// Requests that found no dispatch slot within the queue wait.
    requests_overloaded: AtomicU64,
    /// Requests refused because their repository held its per-repository
    /// in-flight cap for the whole queue wait (QI-BB-002); slots may have
    /// been free for other repositories.
    requests_overloaded_repo: AtomicU64,
    /// Dispatches that had to wait for a slot before they were admitted;
    /// a rising count with a flat `requests_dispatched` is queue pressure.
    dispatch_queue_waits: AtomicU64,
    /// Dispatch slots held right now.
    dispatch_in_flight: AtomicU64,
    /// Response frame bytes written to peers, header included.
    response_bytes: AtomicU64,
    /// Requests that arrived after shutdown began; not dispatched.
    requests_refused_shutting_down: AtomicU64,
    /// Requests the dispatcher answered.
    requests_dispatched: AtomicU64,
    /// Dispatched requests whose peer was gone when the answer was ready.
    peer_hangups: AtomicU64,
    /// Hang-ups the peer watch itself detected mid-dispatch, counted on the
    /// watch thread the moment the hang-up is confirmed (TOPT-02 / R4).
    /// Unlike `peer_hangups` — accounted by the connection thread after its
    /// dispatch returns — this is the event-driven signal tests wait for
    /// instead of sleeping out the watch's poll interval.
    peer_hangup_detected: AtomicU64,
}

/// One consistent-enough read of [`IpcServerCounters`]: each field is
/// atomic, the set is not.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct IpcServerCountersSnapshot {
    pub connections_accepted: u64,
    pub connections_refused: u64,
    pub peers_refused: u64,
    pub peer_credentials_unreadable: u64,
    pub connections_live: u64,
    pub request_decode_failures: u64,
    pub requests_overloaded: u64,
    pub requests_overloaded_repo: u64,
    pub dispatch_queue_waits: u64,
    pub dispatch_in_flight: u64,
    pub response_bytes: u64,
    pub requests_refused_shutting_down: u64,
    pub requests_dispatched: u64,
    pub peer_hangups: u64,
    pub peer_hangup_detected: u64,
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
            peers_refused: AtomicU64::new(0),
            peer_credentials_unreadable: AtomicU64::new(0),
            connections_live: AtomicU64::new(0),
            request_decode_failures: AtomicU64::new(0),
            requests_overloaded: AtomicU64::new(0),
            requests_overloaded_repo: AtomicU64::new(0),
            dispatch_queue_waits: AtomicU64::new(0),
            dispatch_in_flight: AtomicU64::new(0),
            response_bytes: AtomicU64::new(0),
            requests_refused_shutting_down: AtomicU64::new(0),
            requests_dispatched: AtomicU64::new(0),
            peer_hangups: AtomicU64::new(0),
            peer_hangup_detected: AtomicU64::new(0),
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
            peers_refused: self.peers_refused.load(Ordering::Acquire),
            peer_credentials_unreadable: self.peer_credentials_unreadable.load(Ordering::Acquire),
            connections_live: self.connections_live.load(Ordering::Acquire),
            request_decode_failures: self.request_decode_failures.load(Ordering::Acquire),
            requests_overloaded: self.requests_overloaded.load(Ordering::Acquire),
            requests_overloaded_repo: self.requests_overloaded_repo.load(Ordering::Acquire),
            dispatch_queue_waits: self.dispatch_queue_waits.load(Ordering::Acquire),
            dispatch_in_flight: self.dispatch_in_flight.load(Ordering::Acquire),
            response_bytes: self.response_bytes.load(Ordering::Acquire),
            requests_refused_shutting_down: self
                .requests_refused_shutting_down
                .load(Ordering::Acquire),
            requests_dispatched: self.requests_dispatched.load(Ordering::Acquire),
            peer_hangups: self.peer_hangups.load(Ordering::Acquire),
            peer_hangup_detected: self.peer_hangup_detected.load(Ordering::Acquire),
        }
    }

    pub(crate) fn connection_accepted(&self) {
        let _prior = self.connections_accepted.fetch_add(1, Ordering::AcqRel);
        let _prior = self.connections_live.fetch_add(1, Ordering::AcqRel);
    }

    pub(crate) fn connection_refused(&self) {
        let _prior = self.connections_refused.fetch_add(1, Ordering::AcqRel);
    }

    pub(crate) fn peer_refused(&self) {
        let _prior = self.peers_refused.fetch_add(1, Ordering::AcqRel);
    }

    pub(crate) fn peer_credentials_unreadable(&self) {
        let _prior = self
            .peer_credentials_unreadable
            .fetch_add(1, Ordering::AcqRel);
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

    /// A slot refusal: the global bound or, when `repo_scoped`, the
    /// per-repository one.
    pub(crate) fn request_overloaded(&self, repo_scoped: bool) {
        if repo_scoped {
            let _prior = self.requests_overloaded_repo.fetch_add(1, Ordering::AcqRel);
        } else {
            let _prior = self.requests_overloaded.fetch_add(1, Ordering::AcqRel);
        }
    }

    /// A dispatch slot was taken; `waited` says whether the request had to
    /// wait for it.
    pub(crate) fn dispatch_started(&self, waited: bool) {
        if waited {
            let _prior = self.dispatch_queue_waits.fetch_add(1, Ordering::AcqRel);
        }
        let _prior = self.dispatch_in_flight.fetch_add(1, Ordering::AcqRel);
    }

    /// The dispatch slot was released.
    pub(crate) fn dispatch_finished(&self) {
        let _prior = self.dispatch_in_flight.fetch_sub(1, Ordering::AcqRel);
    }

    /// One response frame of `bytes` was written to a peer.
    pub(crate) fn response_written(&self, bytes: u64) {
        let _prior = self.response_bytes.fetch_add(bytes, Ordering::AcqRel);
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

    /// The peer watch confirmed a hang-up mid-dispatch.
    pub(crate) fn peer_hangup_detected(&self) {
        let _prior = self.peer_hangup_detected.fetch_add(1, Ordering::AcqRel);
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
            MetricPointV1::counter(
                self.metric_name("peer_refused_total"),
                snapshot.peers_refused,
            ),
            MetricPointV1::counter(
                self.metric_name("peer_credentials_unreadable_total"),
                snapshot.peer_credentials_unreadable,
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
                self.metric_name("requests_overloaded_repo_total"),
                snapshot.requests_overloaded_repo,
            ),
            MetricPointV1::counter(
                self.metric_name("dispatch_queue_wait_total"),
                snapshot.dispatch_queue_waits,
            ),
            MetricPointV1::gauge_count(
                self.metric_name("dispatch_in_flight"),
                snapshot.dispatch_in_flight,
            ),
            MetricPointV1::counter(
                self.metric_name("response_bytes_total"),
                snapshot.response_bytes,
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
            MetricPointV1::counter(
                self.metric_name("peer_hangup_detected_total"),
                snapshot.peer_hangup_detected,
            ),
        ])
    }
}

/// The RAII live-connection permit (SEP-21 P08 / S21-09).
///
/// `admit` counts the accepted connection and dropping the guard —
/// normal tail, early return, or panic unwind — releases the live
/// count, so `connections_live` returns to its baseline after a
/// panicking connection too.
pub(crate) struct LiveConnectionGuard {
    counters: Arc<IpcServerCounters>,
}

impl LiveConnectionGuard {
    /// Count one admitted connection and hold its live permit.
    pub(crate) fn admit(counters: &Arc<IpcServerCounters>) -> Self {
        counters.connection_accepted();
        Self {
            counters: Arc::clone(counters),
        }
    }
}

impl Drop for LiveConnectionGuard {
    fn drop(&mut self) {
        self.counters.connection_closed();
    }
}

/// The RAII in-flight dispatch permit (SEP-21 P08 / S21-09).
///
/// `start` counts the slot taken and dropping the guard — normal tail,
/// early return, or panic unwind — releases it.
pub(crate) struct InFlightDispatchGuard {
    counters: Arc<IpcServerCounters>,
}

impl InFlightDispatchGuard {
    /// Count one dispatch started (`waited` says whether it had to wait
    /// for the slot) and hold the in-flight permit.
    pub(crate) fn start(counters: &Arc<IpcServerCounters>, waited: bool) -> Self {
        counters.dispatch_started(waited);
        Self {
            counters: Arc::clone(counters),
        }
    }
}

impl Drop for InFlightDispatchGuard {
    fn drop(&mut self) {
        self.counters.dispatch_finished();
    }
}

#[cfg(test)]
mod tests {
    use super::{InFlightDispatchGuard, IpcServerCounters, LiveConnectionGuard};
    use std::sync::Arc;
    use std::sync::atomic::{AtomicU64, Ordering};

    /// The RAII permits reconcile on a panic unwind: both gauges return
    /// to their baseline (S21-09), so a panicking connection or dispatch
    /// cannot leak a live or in-flight count.
    #[test]
    fn raii_permits_reconcile_on_panic() {
        let counters = Arc::new(IpcServerCounters::for_plane("test"));
        let panicked = Arc::new(AtomicU64::new(0));
        let worker_panicked = Arc::clone(&panicked);
        let worker_counters = Arc::clone(&counters);
        let handle = std::thread::Builder::new()
            .spawn(move || {
                let _live = LiveConnectionGuard::admit(&worker_counters);
                let _in_flight = InFlightDispatchGuard::start(&worker_counters, false);
                let _prior = worker_panicked.fetch_add(1, Ordering::AcqRel);
                panic!("a panicking dispatcher unwinds through the permits");
            })
            .expect("the worker spawns");
        let _joined = handle.join();
        assert_eq!(panicked.load(Ordering::Acquire), 1);
        let snapshot = counters.snapshot();
        assert_eq!(snapshot.connections_accepted, 1);
        assert_eq!(
            snapshot.connections_live, 0,
            "the live count reconciled on unwind"
        );
        assert_eq!(
            snapshot.dispatch_in_flight, 0,
            "the in-flight count reconciled on unwind"
        );
    }

    /// The permits also release on the normal tail.
    #[test]
    fn raii_permits_release_on_the_normal_tail() {
        let counters = Arc::new(IpcServerCounters::for_plane("test"));
        {
            let _live = LiveConnectionGuard::admit(&counters);
            let _in_flight = InFlightDispatchGuard::start(&counters, true);
            let snapshot = counters.snapshot();
            assert_eq!(snapshot.connections_live, 1);
            assert_eq!(snapshot.dispatch_in_flight, 1);
            assert_eq!(snapshot.dispatch_queue_waits, 1);
        }
        let snapshot = counters.snapshot();
        assert_eq!(snapshot.connections_live, 0);
        assert_eq!(snapshot.dispatch_in_flight, 0);
    }
}
