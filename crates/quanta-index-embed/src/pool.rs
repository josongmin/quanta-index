//! Supervisor-owned provider attempt pool (S21-09).
//!
//! Every embedding HTTP attempt runs on a tracked thread: the pool bounds
//! how many may be inflight, names each attempt, reaps finished handles,
//! and joins what remains at drain. No attempt thread is ever detached
//! and forgotten — a cancelled budget abandons its *result*, but the
//! thread stays owned here until it ends or the drain deadline escalates
//! it by name. Attempts are HTTP-timeout-bounded, so even an escalated
//! thread self-terminates; the pool just never pretends it joined one
//! it did not.

use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, AtomicU64, Ordering},
};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use quanta_index_contract::SearchPlaneErrorCodeV2;
use quanta_index_core::CoreError;

/// How often [`ProviderAttemptPool::drain`] polls for finished attempts
/// before its deadline.
const DRAIN_POLL_INTERVAL: Duration = Duration::from_millis(10);

/// One tracked attempt thread.
struct TrackedAttempt {
    /// `pool-prefix + sequence`: names the work, never the payload.
    name: String,
    /// `None` once joined.
    handle: Option<JoinHandle<()>>,
}

/// What [`ProviderAttemptPool::drain`] joined and what it gave up on.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProviderAttemptDrainReport {
    /// Attempts joined during this drain.
    pub joined: usize,
    /// Attempt names still running when the deadline passed. Named, not
    /// claimed: the caller reports them as escalated work.
    pub unfinished: Vec<String>,
}

/// The pool every provider attempt thread is spawned through.
pub struct ProviderAttemptPool {
    max_inflight: usize,
    seq: AtomicU64,
    shutdown: AtomicBool,
    inner: Mutex<Vec<TrackedAttempt>>,
}

impl ProviderAttemptPool {
    /// A pool for at most `max_inflight` concurrent attempt threads. A
    /// zero cap admits nothing and is refused.
    pub fn new(max_inflight: usize) -> Result<Self, CoreError> {
        if max_inflight == 0 {
            return Err(CoreError::InvalidContract(
                "provider attempt pool: max inflight must be non-zero".to_string(),
            ));
        }
        Ok(Self {
            max_inflight,
            seq: AtomicU64::new(0),
            shutdown: AtomicBool::new(false),
            inner: Mutex::new(Vec::new()),
        })
    }

    /// Spawn `work` on a tracked attempt thread named `{prefix}-{seq}`.
    ///
    /// Refuses typed when the pool is shut down or every slot is held by
    /// a live attempt — finished handles are reaped first, so a refusal
    /// always means genuinely live work at the cap.
    pub fn spawn_tracked(
        self: &Arc<Self>,
        prefix: &str,
        work: impl FnOnce() + Send + 'static,
    ) -> Result<(), CoreError> {
        if self.shutdown.load(Ordering::Acquire) {
            return Err(CoreError::Typed {
                code: SearchPlaneErrorCodeV2::RequestCancelled,
                message: "provider attempt pool is shut down".to_string(),
            });
        }
        let mut live = self.inner.lock().map_err(|err| {
            CoreError::Storage(format!("provider attempt pool lock poisoned: {err}"))
        })?;
        let _reaped = Self::reap_finished(&mut live);
        if live.len() >= self.max_inflight {
            return Err(CoreError::Typed {
                code: SearchPlaneErrorCodeV2::ServerOverloaded,
                message: "provider attempt pool is at its inflight cap".to_string(),
            });
        }
        let seq = self.seq.fetch_add(1, Ordering::Relaxed);
        let name = format!("{prefix}-{seq}");
        let handle = std::thread::Builder::new()
            .name(name.clone())
            .spawn(work)
            .map_err(|err| {
                CoreError::InvalidContract(format!(
                    "provider attempt pool: could not start attempt thread: {err}"
                ))
            })?;
        live.push(TrackedAttempt {
            name,
            handle: Some(handle),
        });
        drop(live);
        Ok(())
    }

    /// Attempts currently tracked (live plus finished-but-unreaped). For
    /// gauges and tests; reaps finished handles first.
    pub fn live_attempts(&self) -> usize {
        let Ok(mut live) = self.inner.lock() else {
            return 0;
        };
        let _reaped = Self::reap_finished(&mut live);
        live.len()
    }

    /// Stop accepting new attempts. Idempotent; the drain path calls it
    /// before joining, and `spawn_tracked` refuses afterwards.
    pub fn shutdown(&self) {
        self.shutdown.store(true, Ordering::Release);
    }

    /// Shut down and join every tracked attempt until `deadline`.
    ///
    /// Finished attempts are joined and counted; attempts still running
    /// when the deadline passes are named in `unfinished` and left
    /// tracked — they are HTTP-timeout-bounded and self-terminate, and a
    /// later drain reaps them.
    pub fn drain(&self, deadline: Duration) -> ProviderAttemptDrainReport {
        self.shutdown();
        let start = Instant::now();
        let mut joined = 0usize;
        loop {
            let Ok(mut live) = self.inner.lock() else {
                return ProviderAttemptDrainReport {
                    joined,
                    unfinished: Vec::new(),
                };
            };
            joined = joined.saturating_add(Self::reap_finished(&mut live));
            if live.is_empty() {
                return ProviderAttemptDrainReport {
                    joined,
                    unfinished: Vec::new(),
                };
            }
            if start.elapsed() >= deadline {
                let unfinished = live.iter().map(|attempt| attempt.name.clone()).collect();
                return ProviderAttemptDrainReport { joined, unfinished };
            }
            drop(live);
            std::thread::sleep(DRAIN_POLL_INTERVAL);
        }
    }

    /// Join every finished handle in `live`, dropping panics into the
    /// void a finished attempt already reported through its own channel.
    /// Returns how many were reaped.
    fn reap_finished(live: &mut Vec<TrackedAttempt>) -> usize {
        let mut reaped = 0usize;
        live.retain_mut(|attempt| {
            let finished = attempt.handle.as_ref().is_some_and(JoinHandle::is_finished);
            if finished {
                if let Some(handle) = attempt.handle.take() {
                    let _completed_or_panicked = handle.join();
                }
                reaped = reaped.saturating_add(1);
                return false;
            }
            true
        });
        reaped
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc;

    #[test]
    fn zero_cap_is_refused() {
        assert!(ProviderAttemptPool::new(0).is_err());
    }

    #[test]
    fn full_pool_refuses_and_reaps_on_completion() {
        let pool = Arc::new(ProviderAttemptPool::new(1).expect("valid pool"));
        let (done_tx, done_rx) = mpsc::channel::<()>();
        pool.spawn_tracked("attempt", move || {
            let _wait = done_rx.recv();
        })
        .expect("first attempt fits");
        assert_eq!(pool.live_attempts(), 1);
        let refused = pool
            .spawn_tracked("attempt", || {})
            .expect_err("a live attempt at cap refuses");
        let CoreError::Typed { code, .. } = refused else {
            panic!("cap refusal must be typed");
        };
        assert_eq!(code, SearchPlaneErrorCodeV2::ServerOverloaded);

        drop(done_tx);
        let report = pool.drain(Duration::from_secs(5));
        assert_eq!(report.joined, 1);
        assert!(report.unfinished.is_empty());
        assert_eq!(pool.live_attempts(), 0);
    }

    #[test]
    fn shutdown_refuses_new_attempts() {
        let pool = Arc::new(ProviderAttemptPool::new(4).expect("valid pool"));
        pool.shutdown();
        let refused = pool
            .spawn_tracked("attempt", || {})
            .expect_err("shutdown refuses");
        let CoreError::Typed { code, .. } = refused else {
            panic!("shutdown refusal must be typed");
        };
        assert_eq!(code, SearchPlaneErrorCodeV2::RequestCancelled);
    }

    #[expect(
        clippy::indexing_slicing,
        reason = "index follows an exact length assert on the same unfinished vector"
    )]
    #[test]
    fn drain_names_unfinished_attempts_past_the_deadline() {
        let pool = Arc::new(ProviderAttemptPool::new(4).expect("valid pool"));
        let (_hold_tx, hold_rx) = mpsc::channel::<()>();
        pool.spawn_tracked("stuck", move || {
            let _wait = hold_rx.recv();
        })
        .expect("attempt fits");
        let report = pool.drain(Duration::from_millis(50));
        assert_eq!(report.joined, 0);
        assert_eq!(report.unfinished.len(), 1);
        assert!(report.unfinished[0].starts_with("stuck-"));
        // The stuck thread is still owned, not leaked: it holds its slot.
        assert_eq!(pool.live_attempts(), 1);
    }
}
