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

use std::io::Write;
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, AtomicU64, Ordering},
};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use quanta_index_contract::SearchPlaneErrorCodeV2;
use quanta_index_core::{CoreError, ProviderAttemptLeaseV1};

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

#[derive(Default)]
struct PoolState {
    attempts: Vec<TrackedAttempt>,
    /// Join panics are retained even when a gauge call reaps a finished
    /// attempt before shutdown observes it.
    panicked: Vec<String>,
}

/// What [`ProviderAttemptPool::drain`] joined and what it gave up on.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProviderAttemptDrainReport {
    /// Attempts joined during this drain.
    pub joined: usize,
    /// Attempt names still running when the deadline passed. Named, not
    /// claimed: the caller reports them as escalated work.
    pub unfinished: Vec<String>,
    /// Named attempts whose thread panicked when joined, including those
    /// reaped by a gauge or a later admission before this drain.
    pub panicked: Vec<String>,
}

/// A point-in-time prediction from the same registry lock used by attempt
/// admission. Ready does not reserve a slot or attest to remote endpoint health.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ProviderAttemptIngressSnapshot {
    Ready { live: usize, capacity: usize },
    Busy { live: usize, capacity: usize },
    Offline(ProviderAttemptOfflineReason),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ProviderAttemptOfflineReason {
    Shutdown,
    Poisoned,
    Panicked,
    InvalidRegistry,
}

/// The pool every provider attempt thread is spawned through.
pub struct ProviderAttemptPool {
    max_inflight: usize,
    seq: AtomicU64,
    shutdown: AtomicBool,
    inner: Mutex<PoolState>,
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
            inner: Mutex::new(PoolState::default()),
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
        let mut live = self.inner.lock().map_err(|err| {
            CoreError::Storage(format!("provider attempt pool lock poisoned: {err}"))
        })?;
        // Check shutdown under the registry lock used by drain. An attempt
        // waiting for that lock cannot enter after drain observed an empty
        // registry; an already admitted attempt is registered before drain
        // can inspect it.
        if self.shutdown.load(Ordering::Acquire) {
            return Err(CoreError::Typed {
                code: SearchPlaneErrorCodeV2::RequestCancelled,
                message: "provider attempt pool is shut down".to_string(),
            });
        }
        let _reaped = Self::reap_finished(&mut live);
        // A panicked transport may have left its external side effect in an
        // unknown state. Stop admission and retain bounded panic names until
        // the owner drains the pool; otherwise the registry could grow without
        // bound as new calls repeatedly trigger gauge-side reaping.
        if !live.panicked.is_empty() {
            return Err(CoreError::Storage(format!(
                "provider attempt pool observed {} panicked transport attempt(s)",
                live.panicked.len()
            )));
        }
        if live.attempts.len() >= self.max_inflight {
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
        live.attempts.push(TrackedAttempt {
            name,
            handle: Some(handle),
        });
        drop(live);
        Ok(())
    }

    /// Spawn an admitted physical attempt. A bounded native lease stays with
    /// the actual transport thread through success, panic, or abandoned
    /// response; a refusal or OS spawn failure drops it without starting IO.
    /// Standalone callers explicitly pass `None`.
    pub fn spawn_tracked_with_custody(
        self: &Arc<Self>,
        prefix: &str,
        custody: Option<Box<dyn ProviderAttemptLeaseV1>>,
        work: impl FnOnce() + Send + 'static,
    ) -> Result<(), CoreError> {
        self.spawn_tracked(prefix, move || {
            let _custody = custody;
            work();
        })
    }

    /// Attempts currently tracked (live plus finished-but-unreaped). For
    /// gauges and tests; reaps finished handles first.
    pub fn live_attempts(&self) -> usize {
        // Poison does not prove completion. Recover custody exactly as drain
        // does, while admission continues to refuse the poisoned registry.
        let mut live = match self.inner.lock() {
            Ok(live) => live,
            Err(poisoned) => poisoned.into_inner(),
        };
        let _reaped = Self::reap_finished(&mut live);
        live.attempts.len()
    }

    /// Observe the actual local transport ingress under its admission lock.
    /// A later admission can consume a Ready slot; only `spawn_tracked` grants
    /// execution. Poison, panics, and impossible registry state fail closed.
    pub fn ingress_snapshot(&self) -> ProviderAttemptIngressSnapshot {
        use ProviderAttemptIngressSnapshot::{Busy, Offline, Ready};
        use ProviderAttemptOfflineReason::{InvalidRegistry, Panicked, Poisoned, Shutdown};

        let Ok(mut state) = self.inner.lock() else {
            return Offline(Poisoned);
        };
        let _reaped = Self::reap_finished(&mut state);
        if self.shutdown.load(Ordering::Acquire) {
            return Offline(Shutdown);
        }
        if !state.panicked.is_empty() {
            return Offline(Panicked);
        }
        let live = state.attempts.len();
        if live > self.max_inflight
            || state
                .attempts
                .iter()
                .any(|attempt| attempt.handle.is_none())
        {
            return Offline(InvalidRegistry);
        }
        if live == self.max_inflight {
            Busy {
                live,
                capacity: self.max_inflight,
            }
        } else {
            Ready {
                live,
                capacity: self.max_inflight,
            }
        }
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
            let mut live = match self.inner.lock() {
                Ok(live) => live,
                // Poison is not proof that attempts exited. Keep custody
                // and reconcile the handles even after a panicking owner.
                Err(poisoned) => poisoned.into_inner(),
            };
            joined = joined.saturating_add(Self::reap_finished(&mut live));
            if live.attempts.is_empty() {
                return ProviderAttemptDrainReport {
                    joined,
                    unfinished: Vec::new(),
                    panicked: std::mem::take(&mut live.panicked),
                };
            }
            if start.elapsed() >= deadline {
                let unfinished = live
                    .attempts
                    .iter()
                    .map(|attempt| attempt.name.clone())
                    .collect();
                return ProviderAttemptDrainReport {
                    joined,
                    unfinished,
                    panicked: std::mem::take(&mut live.panicked),
                };
            }
            drop(live);
            std::thread::sleep(DRAIN_POLL_INTERVAL);
        }
    }

    /// Preserve every remaining attempt handle after bounded service grace.
    /// The returned report records the original grace result, even if the
    /// reaper later joins the attempts: an overdue shutdown stays non-green.
    /// A named reaper owns this pool until all physical attempts are joined.
    /// If starting it fails, the caller joins synchronously rather than
    /// dropping live handles and their state-root/grant custody.
    pub fn drain_and_handoff(
        self: Arc<Self>,
        grace: Duration,
    ) -> Result<ProviderAttemptDrainReport, CoreError> {
        let grace_report = self.drain(grace);
        if grace_report.unfinished.is_empty() {
            return Ok(grace_report);
        }
        let reaper_pool = Arc::clone(&self);
        let spawned = std::thread::Builder::new()
            .name("provider-attempt-custody-reaper".into())
            .spawn(move || {
                let completed = reaper_pool.drain_to_completion();
                if !completed.panicked.is_empty() {
                    let diagnostic = format!(
                        "provider attempt custody reaper joined panicked attempts: {}\n",
                        completed.panicked.join(",")
                    );
                    let _written = std::io::stderr().lock().write_all(diagnostic.as_bytes());
                }
            });
        match spawned {
            Ok(reaper) => {
                // This dedicated reaper is the final owner of the pool and
                // joins every physical attempt. Its own handle has no state
                // root or provider grant after the pool is fully drained.
                drop(reaper);
                Ok(grace_report)
            }
            Err(error) => {
                let completed = self.drain_to_completion();
                Err(CoreError::Storage(format!(
                    "provider custody reaper spawn failed ({error}); synchronous fallback joined {} attempts, panicked: [{}]; grace unfinished: [{}]",
                    completed.joined,
                    completed.panicked.join(","),
                    grace_report.unfinished.join(",")
                )))
            }
        }
    }

    fn drain_to_completion(&self) -> ProviderAttemptDrainReport {
        let mut joined = 0usize;
        let mut panicked = Vec::new();
        loop {
            let mut step = self.drain(Duration::from_secs(1));
            joined = joined.saturating_add(step.joined);
            panicked.append(&mut step.panicked);
            if step.unfinished.is_empty() {
                return ProviderAttemptDrainReport {
                    joined,
                    unfinished: Vec::new(),
                    panicked,
                };
            }
        }
    }

    /// Join every finished handle and keep panic names for the next drain.
    /// Returns how many were reaped.
    fn reap_finished(state: &mut PoolState) -> usize {
        let PoolState { attempts, panicked } = state;
        let mut reaped = 0usize;
        attempts.retain_mut(|attempt| {
            let finished = attempt.handle.as_ref().is_some_and(JoinHandle::is_finished);
            if finished {
                if let Some(handle) = attempt.handle.take()
                    && handle.join().is_err()
                {
                    panicked.push(attempt.name.clone());
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
    use std::sync::atomic::AtomicUsize;
    use std::sync::mpsc;

    struct DropProbe(Arc<AtomicUsize>);

    impl ProviderAttemptLeaseV1 for DropProbe {}

    impl Drop for DropProbe {
        fn drop(&mut self) {
            let _previous = self.0.fetch_add(1, Ordering::SeqCst);
        }
    }

    fn lease(dropped: &Arc<AtomicUsize>) -> Box<dyn ProviderAttemptLeaseV1> {
        Box::new(DropProbe(Arc::clone(dropped)))
    }

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
    fn ingress_snapshot_tracks_the_admission_registry() {
        let pool = Arc::new(ProviderAttemptPool::new(1).expect("valid pool"));
        assert_eq!(
            pool.ingress_snapshot(),
            ProviderAttemptIngressSnapshot::Ready {
                live: 0,
                capacity: 1
            }
        );
        let (entered, entered_rx) = mpsc::channel::<()>();
        let (release, parked) = mpsc::channel::<()>();
        pool.spawn_tracked("held", move || {
            let _sent = entered.send(());
            let _done = parked.recv();
        })
        .expect("first attempt fits");
        entered_rx.recv().expect("attempt entered");
        assert_eq!(
            pool.ingress_snapshot(),
            ProviderAttemptIngressSnapshot::Busy {
                live: 1,
                capacity: 1
            }
        );
        drop(release);
        let started = Instant::now();
        while pool.ingress_snapshot()
            != (ProviderAttemptIngressSnapshot::Ready {
                live: 0,
                capacity: 1,
            })
        {
            assert!(started.elapsed() < Duration::from_secs(2));
            std::thread::yield_now();
        }
        pool.shutdown();
        assert_eq!(
            pool.ingress_snapshot(),
            ProviderAttemptIngressSnapshot::Offline(ProviderAttemptOfflineReason::Shutdown)
        );
    }

    #[test]
    fn ingress_snapshot_fails_closed_on_panic_and_poison() {
        let pool = Arc::new(ProviderAttemptPool::new(1).expect("valid pool"));
        pool.spawn_tracked("panic", || {
            std::panic::resume_unwind(Box::new("provider panic"));
        })
        .expect("attempt spawned");
        let started = Instant::now();
        while pool.ingress_snapshot()
            != ProviderAttemptIngressSnapshot::Offline(ProviderAttemptOfflineReason::Panicked)
        {
            assert!(started.elapsed() < Duration::from_secs(2));
            std::thread::yield_now();
        }
        assert_eq!(
            pool.ingress_snapshot(),
            ProviderAttemptIngressSnapshot::Offline(ProviderAttemptOfflineReason::Panicked)
        );

        let poisoned = Arc::new(ProviderAttemptPool::new(1).expect("valid pool"));
        let other = Arc::clone(&poisoned);
        let _panic = std::thread::spawn(move || {
            let _held = other.inner.lock().expect("registry lock");
            panic!("poison provider registry");
        })
        .join();
        assert_eq!(
            poisoned.ingress_snapshot(),
            ProviderAttemptIngressSnapshot::Offline(ProviderAttemptOfflineReason::Poisoned)
        );

        let torn = ProviderAttemptPool::new(1).expect("valid pool");
        torn.inner
            .lock()
            .expect("registry lock")
            .attempts
            .push(TrackedAttempt {
                name: "torn".into(),
                handle: None,
            });
        assert_eq!(
            torn.ingress_snapshot(),
            ProviderAttemptIngressSnapshot::Offline(ProviderAttemptOfflineReason::InvalidRegistry)
        );
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

    macro_rules! check {
        ($condition:expr $(, $message:expr)?) => {
            if !$condition {
                return Err(CoreError::InvalidContract(format!(
                    "assertion failed: {}",
                    stringify!($condition)
                )));
            }
        };
    }

    macro_rules! check_eq {
        ($left:expr, $right:expr) => {
            check!($left == $right);
        };
    }

    #[test]
    fn custody_is_refunded_on_pool_refusal_and_physical_completion() -> Result<(), CoreError> {
        let pool = Arc::new(ProviderAttemptPool::new(1)?);
        let dropped = Arc::new(AtomicUsize::new(0));
        let (release, parked) = mpsc::channel::<()>();
        pool.spawn_tracked_with_custody("held", Some(lease(&dropped)), move || {
            let _done = parked.recv();
        })?;
        check!(matches!(
            pool.spawn_tracked_with_custody("refused", Some(lease(&dropped)), || {}),
            Err(CoreError::Typed {
                code: SearchPlaneErrorCodeV2::ServerOverloaded,
                ..
            })
        ));
        check_eq!(dropped.load(Ordering::SeqCst), 1);
        pool.shutdown();
        check!(matches!(
            pool.spawn_tracked_with_custody("stopped", Some(lease(&dropped)), || {}),
            Err(CoreError::Typed {
                code: SearchPlaneErrorCodeV2::RequestCancelled,
                ..
            })
        ));
        check_eq!(dropped.load(Ordering::SeqCst), 2);
        drop(release);
        let drained = pool.drain(Duration::from_secs(5));
        check!(drained.unfinished.is_empty());
        check_eq!(dropped.load(Ordering::SeqCst), 3);
        Ok(())
    }

    #[test]
    fn panicking_physical_attempt_releases_custody() -> Result<(), CoreError> {
        let pool = Arc::new(ProviderAttemptPool::new(1)?);
        let dropped = Arc::new(AtomicUsize::new(0));
        pool.spawn_tracked_with_custody("panic", Some(lease(&dropped)), || {
            std::panic::resume_unwind(Box::new("transport panicked"));
        })?;
        let drained = pool.drain(Duration::from_secs(5));
        check!(drained.unfinished.is_empty());
        check_eq!(drained.panicked.len(), 1);
        check_eq!(dropped.load(Ordering::SeqCst), 1);
        Ok(())
    }

    #[test]
    fn overdue_grace_hands_attempt_handles_to_named_reaper_until_joined() -> Result<(), CoreError> {
        let pool = Arc::new(ProviderAttemptPool::new(1)?);
        let weak = Arc::downgrade(&pool);
        let (release, parked) = mpsc::channel::<()>();
        pool.spawn_tracked("overdue", move || {
            let _released = parked.recv();
        })?;
        let grace = pool.drain_and_handoff(Duration::ZERO)?;
        check_eq!(grace.unfinished.len(), 1);
        check!(weak.upgrade().is_some(), "reaper retains the attempt pool");
        drop(release);
        let started = Instant::now();
        while weak.upgrade().is_some() && started.elapsed() < Duration::from_secs(5) {
            std::thread::sleep(Duration::from_millis(10));
        }
        check!(
            weak.upgrade().is_none(),
            "reaper joined and released the pool"
        );
        Ok(())
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

    #[test]
    fn poisoned_registry_does_not_claim_a_live_attempt_is_drained() {
        let pool = Arc::new(ProviderAttemptPool::new(1).expect("valid pool"));
        let (release_tx, release_rx) = mpsc::channel::<()>();
        pool.spawn_tracked("held", move || {
            let _released = release_rx.recv();
        })
        .expect("attempt fits");
        let poison_pool = Arc::clone(&pool);
        let poison = std::thread::spawn(move || {
            let _registry = poison_pool
                .inner
                .lock()
                .expect("initial registry is healthy");
            panic!("simulate a panicking registry owner");
        });
        assert!(poison.join().is_err());

        assert_eq!(pool.live_attempts(), 1);
        assert!(matches!(
            pool.spawn_tracked("new", || {}),
            Err(CoreError::Storage(_))
        ));

        let overdue = pool.drain(Duration::ZERO);
        assert_eq!(overdue.unfinished.len(), 1);
        release_tx.send(()).expect("attempt is still running");
        let completed = pool.drain(Duration::from_secs(2));
        assert_eq!(completed.joined, 1);
        assert!(completed.unfinished.is_empty());
        assert_eq!(pool.live_attempts(), 0);
    }
}
