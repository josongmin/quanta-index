//! Single-flight, byte-bounded registry of opened sealed generations.
//!
//! Sealed generations are immutable, so an opened handle for one can be
//! shared by every query that pins it. Before this registry every query route
//! called the adapter's `open` (QI-BB-001): each call re-proved the sealed
//! identity, reopened the native index, re-registered tokenizers and
//! re-decoded every text-authority sidecar and metadata snapshot. The cost was
//! proportional to the corpus, paid per request, and paid again by every
//! concurrent request that missed at the same moment.
//!
//! The registry is the one owner of resident query handles in the search
//! plane: adapters expose a cold `open`, the registry decides what stays
//! resident. Five properties are load-bearing and each has a test:
//!
//! - **Single flight.** Concurrent misses on one key run the opener once;
//!   the others wait on that flight and share its outcome — the handle, or
//!   the opener's own typed failure by value, never a rendering of it — so a
//!   cold open never fans out into N identical loads and a coalesced caller
//!   sees the same wire code the opener saw.
//! - **Bounded waits.** A caller that waits on another's flight waits under
//!   its own request budget: past the deadline, or once its peer left, it
//!   receives the typed interruption and the flight lands for whoever is
//!   still waiting.
//! - **Bounded by entries *and* bytes.** A handle carries the adapter's
//!   resident-bytes estimate; eviction is least-recently-used and runs until
//!   both limits hold. A handle larger than the whole budget is served but
//!   not retained — it must not evict everything else to fit.
//! - **Pins survive eviction.** Handles are `Arc`s. Weak tracking remains
//!   after eviction or oversize refusal, so retirement sees live handles
//!   without retaining their bytes. GC must consult [`SnapshotRegistry::retire`]
//!   before deleting a generation.
//! - **Retirement fences flights.** A generation retired while an open for
//!   it is in flight is not admitted when that open lands: `retire` marks
//!   the flight and defers physical deletion without blocking. The landing
//!   handle is dropped and the opener and waiters see `UNKNOWN_GENERATION`.
//!
//! A resident handle is never invalidated by ingest: a sealed generation is
//! immutable (every publish that names one is refused `GENERATION_IMMUTABLE`
//! before a byte is written, QI-BB-030), so it cannot go stale. The only
//! way out of residency besides eviction is retirement, when GC reaps the
//! generation or a repair replaces a damaged track. Activation and restart
//! promote the handles they proved into the registry
//! ([`SnapshotRegistry::promote`]) so the first query after either is a
//! hit, not a second full open.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex, MutexGuard, Weak};
use std::time::Instant;

use quanta_index_contract::{ManifestGeneration, RepoId, RevisionId};
use quanta_index_core::domains::lexical::LexicalSearcher;
use quanta_index_core::domains::semantic::SemanticSearcher;

use crate::single_flight::{AwaitFlightFailure, Flight};
use quanta_index_core::{
    CoreError, MetricPointV1, MetricSourcePort, RequestBudgetV1, count_from_usize,
};

/// The checkpoint name a waiter's typed interruption carries.
const AWAIT_FLIGHT_CHECKPOINT: &str = "snapshot-registry:await-flight";

/// Identity of one opened sealed generation within a track's registry.
#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct SnapshotKey {
    pub repo_id: RepoId,
    pub revision_id: RevisionId,
    pub generation: ManifestGeneration,
}

impl SnapshotKey {
    #[must_use]
    pub fn new(repo_id: &RepoId, revision_id: &RevisionId, generation: ManifestGeneration) -> Self {
        Self {
            repo_id: repo_id.clone(),
            revision_id: revision_id.clone(),
            generation,
        }
    }
}

/// Residency limits for one registry.
///
/// Both limits are enforced together: a registry may hold at most
/// `max_entries` handles whose resident-bytes estimates sum to at most
/// `max_resident_bytes`. A zero in either position would silently turn the
/// registry into a pass-through, so it is refused at construction and the
/// fields are private: every policy in existence is a valid one.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SnapshotRegistryPolicy {
    max_entries: usize,
    max_resident_bytes: u64,
}

impl SnapshotRegistryPolicy {
    /// Deployment default: sixteen handles, one `GiB` of resident estimate.
    pub const DEFAULT: Self = Self {
        max_entries: 16,
        max_resident_bytes: 1 << 30,
    };

    /// Build a policy; zero in either position is a configuration defect,
    /// not a request for a disabled cache.
    pub fn new(max_entries: usize, max_resident_bytes: u64) -> Result<Self, CoreError> {
        if max_entries == 0 || max_resident_bytes == 0 {
            return Err(CoreError::InvalidContract(format!(
                "snapshot registry policy must retain at least one entry and one byte, got max_entries={max_entries} max_resident_bytes={max_resident_bytes}"
            )));
        }
        Ok(Self {
            max_entries,
            max_resident_bytes,
        })
    }

    #[must_use]
    pub const fn max_entries(self) -> usize {
        self.max_entries
    }

    #[must_use]
    pub const fn max_resident_bytes(self) -> u64 {
        self.max_resident_bytes
    }
}

/// A handle the registry can retain: the opened generation plus the bytes
/// the adapter estimates it keeps resident.
pub struct OpenedSnapshot<H: ?Sized> {
    pub handle: Arc<H>,
    pub resident_bytes: u64,
}

/// How one acquire was satisfied.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SnapshotAcquireOutcome {
    /// Served from a resident handle.
    Hit,
    /// This caller ran the opener; the cold open took this long.
    Miss { cold_open_nanos: u128 },
    /// Waited on another caller's flight.
    Coalesced,
}

/// A successful acquire: the shared handle and how it was obtained.
pub struct SnapshotAcquired<H: ?Sized> {
    pub handle: Arc<H>,
    pub outcome: SnapshotAcquireOutcome,
}

/// Whether a promoted handle stayed resident.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SnapshotPromoteOutcome {
    /// The handle is resident; the next acquire of its key is a hit.
    Retained,
    /// The handle exceeded the whole byte budget and was not retained; the
    /// next acquire of its key opens again.
    Oversize,
}

/// Counters and gauges for one registry, snapshotted under the lock.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct SnapshotRegistryStats {
    /// Acquires served from a resident handle.
    pub hits: u64,
    /// Acquires that ran the opener.
    pub misses: u64,
    /// Acquires that waited on another caller's flight instead of opening.
    pub coalesced: u64,
    /// Handles dropped from residency to satisfy the policy.
    pub evictions: u64,
    /// Handles served but not retained because they exceeded the byte budget.
    pub oversize_uncached: u64,
    /// Handles removed because their generation was retired (reaped or
    /// repaired). A sealed generation is immutable, so nothing else ever
    /// removes a resident handle.
    pub retirements: u64,
    /// Cold opens that returned a typed failure.
    pub open_failures: u64,
    /// Opens in flight that a retirement fenced; each landed refused and
    /// admitted nothing.
    pub fenced_in_flight: u64,
    /// Coalesced waits that ended with the waiter's budget interruption.
    pub await_interruptions: u64,
    /// Handles admitted by activation or restart rather than by a query.
    pub promotions: u64,
    /// Total wall time spent inside the opener, in nanoseconds.
    pub cold_open_nanos: u128,
    /// Handles currently resident.
    pub entries: usize,
    /// Sum of resident-bytes estimates of the resident handles.
    pub resident_bytes: u64,
}

/// Outcome of asking the registry to let go of a generation for deletion.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SnapshotRetireOutcome {
    /// Nothing was resident or in flight for the key.
    NotResident,
    /// The registry held the last reference; the handle is now dropped.
    Released,
    /// The registry dropped its reference but live holders or an opening
    /// flight remain; their count is reported so deletion can be deferred.
    StillReferenced { holders: usize },
}

struct Entry<H: ?Sized> {
    handle: Arc<H>,
    resident_bytes: u64,
    last_use: u64,
}

struct RegistryState<H: ?Sized> {
    resident: BTreeMap<SnapshotKey, Entry<H>>,
    in_flight: BTreeMap<SnapshotKey, Arc<Flight<H>>>,
    // Weak references cover handles that outlive eviction or were too large
    // to cache. Retirement must account for them before deleting disk bytes.
    tracked: BTreeMap<SnapshotKey, Vec<Weak<H>>>,
    tracks_since_sweep: u8,
    tick: u64,
    stats: SnapshotRegistryStats,
}

impl<H: ?Sized> RegistryState<H> {
    fn track(&mut self, key: &SnapshotKey, handle: &Arc<H>) {
        let weak = Arc::downgrade(handle);
        let handles = self.tracked.entry(key.clone()).or_default();
        handles.retain(|prior| prior.strong_count() > 0);
        if !handles.iter().any(|prior| prior.ptr_eq(&weak)) {
            handles.push(weak);
        }
        self.tracks_since_sweep = self.tracks_since_sweep.saturating_add(1);
        if self.tracks_since_sweep >= 64 {
            self.tracked.retain(|_, handles| {
                handles.retain(|prior| prior.strong_count() > 0);
                !handles.is_empty()
            });
            self.tracks_since_sweep = 0;
        }
    }

    fn touch(&mut self, key: &SnapshotKey) -> Option<Arc<H>> {
        self.tick = self.tick.saturating_add(1);
        let tick = self.tick;
        self.resident.get_mut(key).map(|entry| {
            entry.last_use = tick;
            Arc::clone(&entry.handle)
        })
    }

    fn evict_until_within(&mut self, policy: SnapshotRegistryPolicy, incoming_bytes: u64) {
        while !self.resident.is_empty()
            && (self.resident.len() >= policy.max_entries
                || self.stats.resident_bytes.saturating_add(incoming_bytes)
                    > policy.max_resident_bytes)
        {
            let Some(victim) = self
                .resident
                .iter()
                .min_by_key(|(_, entry)| entry.last_use)
                .map(|(key, _)| key.clone())
            else {
                return;
            };
            if let Some(removed) = self.resident.remove(&victim) {
                self.stats.resident_bytes = self
                    .stats
                    .resident_bytes
                    .saturating_sub(removed.resident_bytes);
                self.stats.evictions = self.stats.evictions.saturating_add(1);
            }
        }
        self.stats.entries = self.resident.len();
    }

    /// Retain `opened` under the policy; reports whether it was retained.
    fn admit(
        &mut self,
        policy: SnapshotRegistryPolicy,
        key: SnapshotKey,
        opened: &OpenedSnapshot<H>,
    ) -> SnapshotPromoteOutcome {
        self.track(&key, &opened.handle);
        if opened.resident_bytes > policy.max_resident_bytes {
            self.stats.oversize_uncached = self.stats.oversize_uncached.saturating_add(1);
            return SnapshotPromoteOutcome::Oversize;
        }
        // Replacing a resident entry frees its bytes before the budget is
        // measured, so a re-admit of the same key never evicts a neighbour
        // to make room for bytes it is about to give back.
        if let Some(prior) = self.resident.remove(&key) {
            self.stats.resident_bytes = self
                .stats
                .resident_bytes
                .saturating_sub(prior.resident_bytes);
        }
        self.evict_until_within(policy, opened.resident_bytes);
        self.tick = self.tick.saturating_add(1);
        let _none = self.resident.insert(
            key,
            Entry {
                handle: Arc::clone(&opened.handle),
                resident_bytes: opened.resident_bytes,
                last_use: self.tick,
            },
        );
        self.stats.resident_bytes = self
            .stats
            .resident_bytes
            .saturating_add(opened.resident_bytes);
        self.stats.entries = self.resident.len();
        SnapshotPromoteOutcome::Retained
    }

    fn remove(&mut self, key: &SnapshotKey) -> Option<Arc<H>> {
        let removed = self.resident.remove(key)?;
        self.stats.resident_bytes = self
            .stats
            .resident_bytes
            .saturating_sub(removed.resident_bytes);
        self.stats.entries = self.resident.len();
        self.stats.retirements = self.stats.retirements.saturating_add(1);
        Some(removed.handle)
    }

    fn external_holders(&mut self, key: &SnapshotKey, removed: Option<&Arc<H>>) -> usize {
        let Some(handles) = self.tracked.get_mut(key) else {
            return 0;
        };
        let mut holders = 0usize;
        handles.retain(|weak| {
            let Some(handle) = weak.upgrade() else {
                return false;
            };
            let registry_reference =
                usize::from(removed.is_some_and(|resident| Arc::ptr_eq(resident, &handle)));
            // This upgrade and the removed registry reference are not
            // readers; every other strong reference prevents deletion.
            holders = holders.saturating_add(
                Arc::strong_count(&handle)
                    .saturating_sub(1usize.saturating_add(registry_reference)),
            );
            true
        });
        if handles.is_empty() {
            let _empty = self.tracked.remove(key);
        }
        holders
    }
}

/// Registry of opened sealed generations for one track.
pub struct SnapshotRegistry<H: ?Sized> {
    policy: SnapshotRegistryPolicy,
    state: Mutex<RegistryState<H>>,
}

impl<H: ?Sized + Send + Sync + 'static> SnapshotRegistry<H> {
    #[must_use]
    pub const fn new(policy: SnapshotRegistryPolicy) -> Self {
        Self {
            policy,
            state: Mutex::new(RegistryState {
                resident: BTreeMap::new(),
                in_flight: BTreeMap::new(),
                tracked: BTreeMap::new(),
                tracks_since_sweep: 0,
                tick: 0,
                stats: SnapshotRegistryStats {
                    hits: 0,
                    misses: 0,
                    coalesced: 0,
                    evictions: 0,
                    oversize_uncached: 0,
                    retirements: 0,
                    open_failures: 0,
                    fenced_in_flight: 0,
                    await_interruptions: 0,
                    promotions: 0,
                    cold_open_nanos: 0,
                    entries: 0,
                    resident_bytes: 0,
                },
            }),
        }
    }

    #[must_use]
    pub const fn policy(&self) -> SnapshotRegistryPolicy {
        self.policy
    }

    fn lock(&self) -> Result<MutexGuard<'_, RegistryState<H>>, CoreError> {
        self.state
            .lock()
            .map_err(|err| CoreError::Storage(format!("snapshot registry poisoned: {err}")))
    }

    /// Return the handle for `key`, opening it with `open` if it is not
    /// resident and no other caller is already opening it.
    ///
    /// `open` runs outside the registry lock. Its success is retained under
    /// the policy; its failure is delivered by value to every caller that
    /// coalesced on this flight and is not retained, so the next acquire
    /// retries. A coalesced caller waits under `budget`: its deadline or
    /// cancellation ends the wait with the typed interruption while the
    /// flight lands for the others. A flight whose key was retired while it
    /// ran admits nothing and is refused [`quanta_index_core::UNKNOWN_GENERATION_CODE`].
    pub fn acquire(
        &self,
        key: &SnapshotKey,
        budget: &RequestBudgetV1,
        open: impl FnOnce() -> Result<OpenedSnapshot<H>, CoreError>,
    ) -> Result<SnapshotAcquired<H>, CoreError> {
        let flight = {
            let mut state = self.lock()?;
            if let Some(handle) = state.touch(key) {
                state.stats.hits = state.stats.hits.saturating_add(1);
                return Ok(SnapshotAcquired {
                    handle,
                    outcome: SnapshotAcquireOutcome::Hit,
                });
            }
            if let Some(flight) = state.in_flight.get(key).map(Arc::clone) {
                state.stats.coalesced = state.stats.coalesced.saturating_add(1);
                drop(state);
                return match Flight::await_outcome(&flight, budget, AWAIT_FLIGHT_CHECKPOINT) {
                    Ok(handle) => Ok(SnapshotAcquired {
                        handle,
                        outcome: SnapshotAcquireOutcome::Coalesced,
                    }),
                    Err(AwaitFlightFailure::Interrupted(interruption)) => {
                        self.record_await_interruption()?;
                        Err(interruption)
                    }
                    Err(AwaitFlightFailure::Flight(failure)) => Err(failure),
                };
            }
            state.stats.misses = state.stats.misses.saturating_add(1);
            let flight = Arc::new(Flight::new());
            let _prior = state.in_flight.insert(key.clone(), Arc::clone(&flight));
            flight
        };

        let started = Instant::now();
        let (opened, opener_panic) = crate::single_flight::catch_open(open);
        let elapsed = started.elapsed().as_nanos();

        // Settle the flight even if the registry lock is poisoned: a waiter
        // must never be left blocked on an outcome that will not arrive.
        let result = match self.lock() {
            Ok(mut state) => {
                let _removed = state.in_flight.remove(key);
                state.stats.cold_open_nanos = state.stats.cold_open_nanos.saturating_add(elapsed);
                match opened {
                    Ok(_) if flight.is_fenced() => Err(retired_in_flight(key)),
                    Ok(opened) => {
                        let _retained = state.admit(self.policy, key.clone(), &opened);
                        Ok(opened.handle)
                    }
                    Err(err) => {
                        state.stats.open_failures = state.stats.open_failures.saturating_add(1);
                        Err(err)
                    }
                }
            }
            Err(poisoned) => Err(poisoned),
        };

        let settled = flight.settle(result.clone());
        if let Some(payload) = opener_panic {
            std::panic::resume_unwind(payload);
        }
        settled?;
        result.map(|handle| SnapshotAcquired {
            handle,
            outcome: SnapshotAcquireOutcome::Miss {
                cold_open_nanos: elapsed,
            },
        })
    }

    fn record_await_interruption(&self) -> Result<(), CoreError> {
        let mut state = self.lock()?;
        state.stats.await_interruptions = state.stats.await_interruptions.saturating_add(1);
        drop(state);
        Ok(())
    }

    /// Retain a handle the caller proved outside the registry (activation,
    /// restart) so the next acquire of `key` is a hit. Byte-accounted like
    /// any admitted handle: it may evict, and an oversize handle is not
    /// retained. A flight in progress for the key is left alone; its
    /// landing re-admits the same generation.
    pub fn promote(
        &self,
        key: &SnapshotKey,
        opened: &OpenedSnapshot<H>,
    ) -> Result<SnapshotPromoteOutcome, CoreError> {
        let mut state = self.lock()?;
        state.stats.promotions = state.stats.promotions.saturating_add(1);
        Ok(state.admit(self.policy, key.clone(), opened))
    }

    /// Drop the registry's reference to `key`, fence any open in flight for
    /// it, and report whether anything else still holds the handle.
    ///
    /// A fenced flight defers deletion without waiting for its opener. When
    /// it lands its handle is refused and dropped rather than admitted.
    /// Live handles remain visible even after eviction or oversize refusal.
    /// Callers intending to delete bytes must not proceed on `StillReferenced`.
    pub fn retire(&self, key: &SnapshotKey) -> Result<SnapshotRetireOutcome, CoreError> {
        let mut state = self.lock()?;
        let removed = state.remove(key);
        let fenced = state.in_flight.get(key).cloned();
        if let Some(flight) = &fenced {
            if flight.fence() {
                state.stats.fenced_in_flight = state.stats.fenced_in_flight.saturating_add(1);
            }
        }
        let holders = state
            .external_holders(key, removed.as_ref())
            .saturating_add(usize::from(fenced.is_some()));
        drop(state);
        Ok(if holders > 0 {
            SnapshotRetireOutcome::StillReferenced { holders }
        } else if removed.is_some() {
            SnapshotRetireOutcome::Released
        } else {
            SnapshotRetireOutcome::NotResident
        })
    }

    pub fn stats(&self) -> Result<SnapshotRegistryStats, CoreError> {
        Ok(self.lock()?.stats)
    }
}

/// The refusal a flight lands with when its key was retired while it ran.
fn retired_in_flight(key: &SnapshotKey) -> CoreError {
    CoreError::Typed {
        code: quanta_index_contract::SearchPlaneErrorCodeV2::UnknownGeneration,
        message: format!(
            "snapshot registry: generation {} of repo={} revision={} was retired while its open was in flight; the durable authority no longer retains it and nothing was admitted",
            key.generation.get(),
            key.repo_id.as_str(),
            key.revision_id.as_str(),
        ),
    }
}

/// The two per-track registries the search plane shares between its query
/// side (which acquires), its activation path (which promotes) and its GC
/// (which retires).
///
/// One policy governs both: the byte budget is a process-level resource and
/// splitting it per track would only move the misconfiguration.
#[derive(Clone)]
pub struct SnapshotRegistries {
    pub lexical: Arc<SnapshotRegistry<dyn LexicalSearcher>>,
    pub semantic: Arc<SnapshotRegistry<dyn SemanticSearcher>>,
}

impl SnapshotRegistries {
    #[must_use]
    pub fn new(policy: SnapshotRegistryPolicy) -> Self {
        Self {
            lexical: Arc::new(SnapshotRegistry::new(policy)),
            semantic: Arc::new(SnapshotRegistry::new(policy)),
        }
    }
}

/// One track's registry stats as scrape points, `snapshot_registry_<track>_…`
/// (QI-BB-015).
fn registry_metric_points(track: &str, stats: &SnapshotRegistryStats) -> Vec<MetricPointV1> {
    let name = |suffix: &str| format!("snapshot_registry_{track}_{suffix}");
    vec![
        MetricPointV1::counter(name("hits_total"), stats.hits),
        MetricPointV1::counter(name("misses_total"), stats.misses),
        MetricPointV1::counter(name("coalesced_total"), stats.coalesced),
        MetricPointV1::counter(name("evictions_total"), stats.evictions),
        MetricPointV1::counter(name("oversize_uncached_total"), stats.oversize_uncached),
        MetricPointV1::counter(name("retirements_total"), stats.retirements),
        MetricPointV1::counter(name("open_failures_total"), stats.open_failures),
        MetricPointV1::counter(name("fenced_in_flight_total"), stats.fenced_in_flight),
        MetricPointV1::counter(name("await_interruptions_total"), stats.await_interruptions),
        MetricPointV1::counter(name("promotions_total"), stats.promotions),
        MetricPointV1::counter(
            name("cold_open_nanos_total"),
            u64::try_from(stats.cold_open_nanos).map_or(u64::MAX, |nanos| nanos),
        ),
        MetricPointV1::gauge_count(name("entries"), count_from_usize(stats.entries)),
        MetricPointV1::gauge_count(name("resident_bytes"), stats.resident_bytes),
    ]
}

impl MetricSourcePort for SnapshotRegistries {
    fn scrape(&self) -> Result<Vec<MetricPointV1>, CoreError> {
        let mut points = registry_metric_points("lexical", &self.lexical.stats()?);
        points.extend(registry_metric_points("semantic", &self.semantic.stats()?));
        Ok(points)
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::mpsc;
    use std::sync::{Arc, Barrier};
    use std::thread;
    use std::time::{Duration, Instant};

    use quanta_index_contract::{ManifestGeneration, RepoId, RevisionId};
    use quanta_index_core::{
        CoreError, REQUEST_CANCELLED_CODE, REQUEST_DEADLINE_EXCEEDED_CODE, RequestBudgetV1,
        UNKNOWN_GENERATION_CODE,
    };

    use super::{
        OpenedSnapshot, SnapshotKey, SnapshotPromoteOutcome, SnapshotRegistry,
        SnapshotRegistryPolicy, SnapshotRetireOutcome,
    };

    type TestResult = Result<(), Box<dyn std::error::Error>>;

    #[derive(Debug)]
    struct Handle {
        key: SnapshotKey,
    }

    fn key(generation: u64) -> SnapshotKey {
        SnapshotKey::new(
            &RepoId::new("repo").expect("static fixture ID satisfies canonical policy"),
            &RevisionId::new("rev").expect("static fixture ID satisfies canonical policy"),
            ManifestGeneration::new(generation),
        )
    }

    fn policy(max_entries: usize, max_resident_bytes: u64) -> SnapshotRegistryPolicy {
        SnapshotRegistryPolicy::new(max_entries, max_resident_bytes)
            .expect("test policies are nonzero")
    }

    /// Acquire under an unbounded budget and keep only the handle;
    /// outcome-specific tests inspect the registry stats instead.
    fn get(
        registry: &SnapshotRegistry<Handle>,
        key: &SnapshotKey,
        open: impl FnOnce() -> Result<OpenedSnapshot<Handle>, CoreError>,
    ) -> Result<Arc<Handle>, CoreError> {
        registry
            .acquire(key, &RequestBudgetV1::unbounded(), open)
            .map(|acquired| acquired.handle)
    }

    fn opened(key: &SnapshotKey, bytes: u64) -> OpenedSnapshot<Handle> {
        OpenedSnapshot {
            handle: Arc::new(Handle { key: key.clone() }),
            resident_bytes: bytes,
        }
    }

    /// Spin until `condition` holds or `limit` passes; the registry's
    /// stats are the observable a test waits on, never a sleep.
    fn wait_until(limit: Duration, mut condition: impl FnMut() -> bool) -> bool {
        let started = Instant::now();
        while !condition() {
            if started.elapsed() > limit {
                return false;
            }
            thread::sleep(Duration::from_millis(1));
        }
        true
    }

    #[test]
    fn second_acquire_of_the_same_key_does_not_open_again() -> TestResult {
        let registry = SnapshotRegistry::new(policy(4, 1_000));
        let opens = AtomicUsize::new(0);
        let first = get(&registry, &key(1), || {
            let _count = opens.fetch_add(1, Ordering::SeqCst);
            Ok(opened(&key(1), 10))
        })?;
        let second = get(&registry, &key(1), || {
            let _count = opens.fetch_add(1, Ordering::SeqCst);
            Ok(opened(&key(1), 10))
        })?;
        if opens.load(Ordering::SeqCst) != 1 {
            return Err(format!("opened {} times", opens.load(Ordering::SeqCst)).into());
        }
        if !Arc::ptr_eq(&first, &second) {
            return Err("second acquire returned a different handle".into());
        }
        let stats = registry.stats()?;
        if stats.hits != 1 || stats.misses != 1 || stats.entries != 1 || stats.resident_bytes != 10
        {
            return Err(format!("unexpected stats {stats:?}").into());
        }
        Ok(())
    }

    /// Thirty-two threads miss on one key at the same instant; the opener
    /// runs once and every thread receives the same handle.
    #[test]
    fn concurrent_misses_on_one_key_open_once() -> TestResult {
        const THREADS: usize = 32;
        let registry = Arc::new(SnapshotRegistry::new(policy(4, 1_000)));
        let opens = Arc::new(AtomicUsize::new(0));
        let barrier = Arc::new(Barrier::new(THREADS));
        let mut joins = Vec::with_capacity(THREADS);
        for _ in 0..THREADS {
            let registry = Arc::clone(&registry);
            let opens = Arc::clone(&opens);
            let barrier = Arc::clone(&barrier);
            joins.push(thread::spawn(move || -> Result<Arc<Handle>, CoreError> {
                let _arrived = barrier.wait();
                get(&registry, &key(7), || {
                    let _count = opens.fetch_add(1, Ordering::SeqCst);
                    // Hold the flight open long enough for every peer to arrive.
                    thread::sleep(Duration::from_millis(50));
                    Ok(opened(&key(7), 1))
                })
            }));
        }
        let mut handles = Vec::with_capacity(THREADS);
        for join in joins {
            handles.push(join.join().map_err(|_panic| "acquire thread panicked")??);
        }
        if opens.load(Ordering::SeqCst) != 1 {
            return Err(
                format!("opened {} times for one key", opens.load(Ordering::SeqCst)).into(),
            );
        }
        let Some(first) = handles.first() else {
            return Err("no handles".into());
        };
        if !handles.iter().all(|handle| Arc::ptr_eq(handle, first)) {
            return Err("coalesced acquires returned different handles".into());
        }
        let stats = registry.stats()?;
        if stats.misses != 1 || stats.coalesced != u64::try_from(THREADS.saturating_sub(1))? {
            return Err(format!("unexpected stats {stats:?}").into());
        }
        Ok(())
    }

    /// A failed flight fails every coalesced waiter with the opener's own
    /// typed error — same variant, same message — and retains nothing, so
    /// the next acquire retries the opener.
    #[test]
    fn a_failed_open_is_shared_with_waiters_by_value_and_not_retained() -> TestResult {
        let registry = Arc::new(SnapshotRegistry::new(policy(4, 1_000)));
        let entered = Arc::new(Barrier::new(2));
        let release = Arc::new(Barrier::new(2));
        let opener = {
            let registry = Arc::clone(&registry);
            let entered = Arc::clone(&entered);
            let release = Arc::clone(&release);
            thread::spawn(move || -> Result<Arc<Handle>, CoreError> {
                get(&registry, &key(3), || {
                    let _arrived = entered.wait();
                    let _released = release.wait();
                    Err(CoreError::NotFound("generation vanished".to_string()))
                })
            })
        };
        // The flight is in progress once the opener passed `entered`.
        let _arrived = entered.wait();
        let waiter = {
            let registry = Arc::clone(&registry);
            thread::spawn(move || -> Result<Arc<Handle>, CoreError> {
                get(&registry, &key(3), || Ok(opened(&key(3), 1)))
            })
        };
        if !wait_until(Duration::from_secs(5), || {
            registry.stats().is_ok_and(|stats| stats.coalesced == 1)
        }) {
            return Err("the waiter did not coalesce on the flight".into());
        }
        let _released = release.wait();
        let opener_outcome = opener.join().map_err(|_panic| "opener panicked")?;
        let waiter_outcome = waiter.join().map_err(|_panic| "waiter panicked")?;
        match opener_outcome {
            Err(CoreError::NotFound(message)) if message == "generation vanished" => {}
            other => return Err(format!("opener must fail typed: {other:?}").into()),
        }
        match waiter_outcome {
            Err(CoreError::NotFound(message)) if message == "generation vanished" => {}
            other => {
                return Err(format!(
                    "a coalesced waiter must receive the opener's typed error by value: {other:?}"
                )
                .into());
            }
        }
        let stats = registry.stats()?;
        if stats.entries != 0 || stats.open_failures != 1 {
            return Err(format!("a failed flight must not be retained: {stats:?}").into());
        }
        let retried = get(&registry, &key(3), || Ok(opened(&key(3), 1)))?;
        if retried.key != key(3) {
            return Err("retry after failure did not open".into());
        }
        Ok(())
    }

    /// A coalesced waiter waits under its own budget: a deadline that
    /// passes, or a peer that leaves, ends the wait with the typed
    /// interruption while the flight still lands and is retained.
    #[test]
    fn a_coalesced_wait_observes_its_budget() -> TestResult {
        let registry = Arc::new(SnapshotRegistry::new(policy(4, 1_000)));
        let entered = Arc::new(Barrier::new(2));
        let release = Arc::new(Barrier::new(2));
        let opener = {
            let registry = Arc::clone(&registry);
            let entered = Arc::clone(&entered);
            let release = Arc::clone(&release);
            thread::spawn(move || -> Result<Arc<Handle>, CoreError> {
                get(&registry, &key(5), || {
                    let _arrived = entered.wait();
                    let _released = release.wait();
                    Ok(opened(&key(5), 1))
                })
            })
        };
        let _arrived = entered.wait();
        let deadline_waiter = {
            let registry = Arc::clone(&registry);
            thread::spawn(move || {
                registry
                    .acquire(
                        &key(5),
                        &RequestBudgetV1::for_duration(Duration::from_millis(30)),
                        || Ok(opened(&key(5), 1)),
                    )
                    .map(|acquired| acquired.handle)
            })
        };
        let cancelled_budget = RequestBudgetV1::for_duration(Duration::from_secs(60));
        let cancel = cancelled_budget.cancel_handle();
        let cancelled_waiter = {
            let registry = Arc::clone(&registry);
            thread::spawn(move || {
                registry
                    .acquire(&key(5), &cancelled_budget, || Ok(opened(&key(5), 1)))
                    .map(|acquired| acquired.handle)
            })
        };
        if !wait_until(Duration::from_secs(5), || {
            registry.stats().is_ok_and(|stats| stats.coalesced == 2)
        }) {
            return Err("the waiters did not coalesce on the flight".into());
        }
        cancel.cancel();
        match deadline_waiter.join().map_err(|_panic| "waiter panicked")? {
            Err(CoreError::Typed { code, message }) if code == REQUEST_DEADLINE_EXCEEDED_CODE => {
                if !message.contains("snapshot-registry:await-flight") {
                    return Err(format!("interruption names no checkpoint: {message}").into());
                }
            }
            other => return Err(format!("deadline waiter must be interrupted: {other:?}").into()),
        }
        match cancelled_waiter
            .join()
            .map_err(|_panic| "waiter panicked")?
        {
            Err(CoreError::Typed { code, .. }) if code == REQUEST_CANCELLED_CODE => {}
            other => return Err(format!("cancelled waiter must be interrupted: {other:?}").into()),
        }
        // The flight is still open; releasing it lands the handle for
        // whoever asks next.
        let _released = release.wait();
        let landed = opener.join().map_err(|_panic| "opener panicked")??;
        let stats = registry.stats()?;
        if stats.entries != 1 || stats.await_interruptions != 2 || stats.open_failures != 0 {
            return Err(format!("the flight must land after its waiters left: {stats:?}").into());
        }
        let hit = get(&registry, &key(5), || {
            Err(CoreError::Storage("must hit".into()))
        })?;
        if !Arc::ptr_eq(&landed, &hit) {
            return Err("the landed handle is not the resident one".into());
        }
        Ok(())
    }

    /// Retiring an opening key defers deletion without waiting for a stuck
    /// opener; the eventual landing is refused `UNKNOWN_GENERATION`.
    #[test]
    fn retire_fences_an_open_in_flight_without_waiting_for_it() -> TestResult {
        let registry = Arc::new(SnapshotRegistry::new(policy(4, 1_000)));
        let entered = Arc::new(Barrier::new(2));
        let release = Arc::new(Barrier::new(2));
        let opens = Arc::new(AtomicUsize::new(0));
        let opener = {
            let registry = Arc::clone(&registry);
            let entered = Arc::clone(&entered);
            let release = Arc::clone(&release);
            let opens = Arc::clone(&opens);
            thread::spawn(move || -> Result<Arc<Handle>, CoreError> {
                get(&registry, &key(9), || {
                    let _count = opens.fetch_add(1, Ordering::SeqCst);
                    let _arrived = entered.wait();
                    let _released = release.wait();
                    Ok(opened(&key(9), 1))
                })
            })
        };
        let _arrived = entered.wait();
        let (retired_tx, retired_rx) = mpsc::channel();
        let retirer = {
            let registry = Arc::clone(&registry);
            thread::spawn(move || {
                let outcome = registry.retire(&key(9));
                let _sent = retired_tx.send(outcome);
            })
        };
        // The opener remains held at `release`; retirement must finish
        // without waiting for that potentially non-returning callback.
        if !wait_until(Duration::from_secs(5), || {
            registry
                .stats()
                .is_ok_and(|stats| stats.fenced_in_flight == 1)
        }) {
            return Err("retire did not fence the open in flight".into());
        }
        let promptly_retired = retired_rx.recv_timeout(Duration::from_millis(500));
        let repeated = if promptly_retired.is_ok() {
            Some(registry.retire(&key(9))?)
        } else {
            None
        };
        let _released = release.wait();
        retirer.join().map_err(|_panic| "retirer panicked")?;
        let retire_outcome = promptly_retired
            .map_err(|_timeout| "retire waited for the opener instead of deferring deletion")??;
        if repeated != Some(SnapshotRetireOutcome::StillReferenced { holders: 1 }) {
            return Err("a second retire did not defer the same pending flight".into());
        }
        let opener_outcome = opener.join().map_err(|_panic| "opener panicked")?;
        if retire_outcome != (SnapshotRetireOutcome::StillReferenced { holders: 1 }) {
            return Err(format!("retire must report the fenced flight: {retire_outcome:?}").into());
        }
        match opener_outcome {
            Err(CoreError::Typed { code, .. }) if code == UNKNOWN_GENERATION_CODE => {}
            other => {
                return Err(format!("the fenced opener must be refused typed: {other:?}").into());
            }
        }
        let stats = registry.stats()?;
        if stats.entries != 0 || stats.fenced_in_flight != 1 || opens.load(Ordering::SeqCst) != 1 {
            return Err(format!("a fenced flight must admit nothing: {stats:?}").into());
        }
        if registry.retire(&key(9))? != SnapshotRetireOutcome::NotResident {
            return Err("settled fenced flight did not clear its deferred retirement".into());
        }
        // Nothing is resident and nothing is fenced any more: the next
        // acquire opens afresh.
        let reopened = get(&registry, &key(9), || {
            let _count = opens.fetch_add(1, Ordering::SeqCst);
            Ok(opened(&key(9), 1))
        })?;
        if reopened.key != key(9) || opens.load(Ordering::SeqCst) != 2 {
            return Err("the key must open afresh after the fence lifted".into());
        }
        Ok(())
    }

    #[test]
    #[expect(
        clippy::panic_in_result_fn,
        reason = "the cold-open callback must panic to exercise flight cleanup during unwinding"
    )]
    fn panicked_opener_releases_the_flight_for_a_later_acquire() -> TestResult {
        let registry = SnapshotRegistry::new(policy(4, 1_000));
        let key = key(10);
        let panicked = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _opened = get(&registry, &key, || panic!("injected cold-open panic"));
        }));
        if panicked.is_ok() {
            return Err("the injected cold-open panic must propagate".into());
        }
        if registry.retire(&key)? != SnapshotRetireOutcome::NotResident {
            return Err("a panicked open must not leave a flight for retirement to wait on".into());
        }
        let acquired = registry.acquire(
            &key,
            &RequestBudgetV1::for_duration(Duration::from_millis(50)),
            || Ok(opened(&key, 1)),
        )?;
        if acquired.handle.key != key {
            return Err("a fresh cold open did not replace the panicked flight".into());
        }
        Ok(())
    }

    /// A promoted handle is served as a hit; the opener never runs.
    #[test]
    fn a_promoted_handle_makes_the_next_acquire_a_hit() -> TestResult {
        let registry = SnapshotRegistry::new(policy(4, 1_000));
        let promoted = opened(&key(2), 40);
        if registry.promote(&key(2), &promoted)? != SnapshotPromoteOutcome::Retained {
            return Err("a handle within budget must be retained".into());
        }
        let served = get(&registry, &key(2), || {
            Err(CoreError::Storage("must hit".into()))
        })?;
        if !Arc::ptr_eq(&served, &promoted.handle) {
            return Err("the promoted handle was not the one served".into());
        }
        let stats = registry.stats()?;
        if stats.hits != 1
            || stats.misses != 0
            || stats.promotions != 1
            || stats.resident_bytes != 40
        {
            return Err(format!("promotion stats drifted: {stats:?}").into());
        }
        // Promotion is byte-accounted: an oversize handle is not retained.
        let oversize = opened(&key(3), 1_001);
        if registry.promote(&key(3), &oversize)? != SnapshotPromoteOutcome::Oversize {
            return Err("an oversize handle must not be retained".into());
        }
        if registry.stats()?.entries != 1 {
            return Err("an oversize promotion changed residency".into());
        }
        Ok(())
    }

    #[test]
    fn eviction_is_least_recently_used_and_honors_both_limits() -> TestResult {
        let registry = SnapshotRegistry::new(policy(3, 100));
        for generation in 1..=3 {
            let _handle = get(&registry, &key(generation), || {
                Ok(opened(&key(generation), 30))
            })?;
        }
        // Touch 1 so 2 becomes the least recently used.
        let _touch = get(&registry, &key(1), || {
            Err(CoreError::Storage("must hit".into()))
        })?;
        // Entry limit: the fourth key evicts exactly one entry, key 2.
        let _fourth = get(&registry, &key(4), || Ok(opened(&key(4), 30)))?;
        let stats = registry.stats()?;
        if stats.entries != 3 || stats.evictions != 1 || stats.resident_bytes != 90 {
            return Err(format!("entry-limit eviction drifted: {stats:?}").into());
        }
        if registry.retire(&key(2))? != SnapshotRetireOutcome::NotResident {
            return Err("evicted key without a live handle was not retired".into());
        }
        // Byte limit: a 60-byte entry needs 90 + 60 <= 100 -> evicts until it fits.
        let _big = get(&registry, &key(5), || Ok(opened(&key(5), 60)))?;
        let stats = registry.stats()?;
        if stats.resident_bytes > 100 || stats.entries > 3 {
            return Err(format!("byte-limit eviction drifted: {stats:?}").into());
        }
        Ok(())
    }

    #[test]
    fn an_oversize_handle_is_served_but_not_retained() -> TestResult {
        let registry = SnapshotRegistry::new(policy(3, 100));
        let _small = get(&registry, &key(1), || Ok(opened(&key(1), 10)))?;
        let big = get(&registry, &key(2), || Ok(opened(&key(2), 101)))?;
        if big.key != key(2) {
            return Err("oversize handle was not served".into());
        }
        let stats = registry.stats()?;
        if stats.entries != 1 || stats.oversize_uncached != 1 || stats.evictions != 0 {
            return Err(format!("oversize handling drifted: {stats:?}").into());
        }
        Ok(())
    }

    #[test]
    fn retirement_defers_for_uncached_oversize_handle() -> TestResult {
        let registry = SnapshotRegistry::new(policy(1, 100));
        let held = get(&registry, &key(6), || Ok(opened(&key(6), 101)))?;
        if registry.retire(&key(6))? != (SnapshotRetireOutcome::StillReferenced { holders: 1 }) {
            return Err("retirement treated a live oversize handle as absent".into());
        }
        drop(held);
        if registry.retire(&key(6))? != SnapshotRetireOutcome::NotResident {
            return Err("retirement retained an already dropped oversize handle".into());
        }
        Ok(())
    }

    #[test]
    fn retirement_counts_both_live_incarnations_after_eviction() -> TestResult {
        let registry = SnapshotRegistry::new(policy(1, 100));
        let old = get(&registry, &key(6), || Ok(opened(&key(6), 1)))?;
        let evicting = get(&registry, &key(7), || Ok(opened(&key(7), 1)))?;
        drop(evicting);
        let new = get(&registry, &key(6), || Ok(opened(&key(6), 1)))?;
        if registry.retire(&key(6))? != (SnapshotRetireOutcome::StillReferenced { holders: 2 }) {
            return Err("retirement missed the evicted incarnation of the same key".into());
        }
        drop(old);
        if registry.retire(&key(6))? != (SnapshotRetireOutcome::StillReferenced { holders: 1 }) {
            return Err("retirement lost the newer live incarnation".into());
        }
        drop(new);
        if registry.retire(&key(6))? != SnapshotRetireOutcome::NotResident {
            return Err("retirement retained dead incarnations".into());
        }
        Ok(())
    }

    #[test]
    fn weak_tracking_does_not_grow_with_historical_misses() -> TestResult {
        let registry = SnapshotRegistry::new(policy(1, 100));
        for generation in 1..=128 {
            let held = get(&registry, &key(generation), || {
                Ok(opened(&key(generation), 1))
            })?;
            drop(held);
        }
        let tracked = registry.lock()?.tracked.len();
        if tracked > 64 {
            return Err(format!("dead weak tracking grew with all old misses: {tracked}").into());
        }
        Ok(())
    }

    /// An evicted or invalidated handle stays usable by whoever holds it,
    /// and `retire` reports those holders instead of pretending the bytes
    /// are free.
    #[test]
    fn eviction_and_retire_do_not_invalidate_handles_in_flight() -> TestResult {
        let registry = SnapshotRegistry::new(policy(1, 1_000));
        let held = get(&registry, &key(1), || Ok(opened(&key(1), 1)))?;
        // Entry limit 1: opening key 2 evicts key 1 from residency.
        let other_handle = get(&registry, &key(2), || Ok(opened(&key(2), 1)))?;
        if held.key != key(1) {
            return Err("evicted handle became unusable".into());
        }
        match registry.retire(&key(1))? {
            SnapshotRetireOutcome::StillReferenced { holders: 1 } => {}
            reported @ (SnapshotRetireOutcome::Released
            | SnapshotRetireOutcome::NotResident
            | SnapshotRetireOutcome::StillReferenced { .. }) => {
                return Err(format!("evicted key reported {reported:?}").into());
            }
        }
        drop(held);
        if registry.retire(&key(1))? != SnapshotRetireOutcome::NotResident {
            return Err("dropped evicted handle still blocked retirement".into());
        }
        let held_two = get(&registry, &key(2), || {
            Err(CoreError::Storage("must hit".into()))
        })?;
        match registry.retire(&key(2))? {
            SnapshotRetireOutcome::StillReferenced { holders: 2 } => {}
            reported @ (SnapshotRetireOutcome::NotResident
            | SnapshotRetireOutcome::Released
            | SnapshotRetireOutcome::StillReferenced { .. }) => {
                return Err(format!("held key reported {reported:?}").into());
            }
        }
        drop(held_two);
        drop(other_handle);
        let reopened = get(&registry, &key(2), || Ok(opened(&key(2), 1)))?;
        match registry.retire(&key(2))? {
            SnapshotRetireOutcome::StillReferenced { holders: 1 } => {}
            reported @ (SnapshotRetireOutcome::NotResident
            | SnapshotRetireOutcome::Released
            | SnapshotRetireOutcome::StillReferenced { .. }) => {
                return Err(format!("reopened key reported {reported:?}").into());
            }
        }
        drop(reopened);
        let fresh = get(&registry, &key(3), || Ok(opened(&key(3), 1)))?;
        drop(fresh);
        match registry.retire(&key(3))? {
            SnapshotRetireOutcome::Released => Ok(()),
            reported @ (SnapshotRetireOutcome::NotResident
            | SnapshotRetireOutcome::StillReferenced { .. }) => {
                Err(format!("unreferenced key reported {reported:?}").into())
            }
        }
    }

    #[test]
    fn zero_limits_are_refused_at_construction() {
        assert!(SnapshotRegistryPolicy::new(0, 10).is_err());
        assert!(SnapshotRegistryPolicy::new(1, 0).is_err());
        assert!(SnapshotRegistryPolicy::new(1, 1).is_ok());
    }
}
