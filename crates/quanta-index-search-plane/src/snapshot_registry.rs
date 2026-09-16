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
//! resident. Three properties are load-bearing and each has a test:
//!
//! - **Single flight.** Concurrent misses on one key run the opener once;
//!   the others wait on that flight and share its outcome (success or typed
//!   failure), so a cold open never fans out into N identical loads.
//! - **Bounded by entries *and* bytes.** A handle carries the adapter's
//!   resident-bytes estimate; eviction is least-recently-used and runs until
//!   both limits hold. A handle larger than the whole budget is served but
//!   not retained — it must not evict everything else to fit.
//! - **Pins survive eviction.** Handles are `Arc`s. Evicting or invalidating
//!   an entry drops the registry's reference only; a query mid-flight keeps
//!   its handle alive and the native files it maps. Physical deletion of a
//!   generation's bytes (W3 GC) must therefore consult [`Self::retire`], which
//!   reports whether anything still references the handle.
//!
//! Invalidation is explicit. A sealed generation never goes stale by itself;
//! it is invalidated when a mutation the search plane routed touches the
//! same generation key (auxiliary snapshots published after seal, discard,
//! retirement). The ingest side owns those calls.

use std::collections::BTreeMap;
use std::sync::{Arc, Condvar, Mutex};
use std::time::Instant;

use quanta_index_contract::{ManifestGeneration, RepoId, RevisionId};
use quanta_index_core::CoreError;
use quanta_index_core::domains::lexical::LexicalSearcher;
use quanta_index_core::domains::semantic::SemanticSearcher;

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
    /// Handles removed by explicit invalidation or retirement.
    pub invalidations: u64,
    /// Cold opens that returned a typed failure.
    pub open_failures: u64,
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
    /// Nothing was resident for the key.
    NotResident,
    /// The registry held the last reference; the handle is now dropped.
    Released,
    /// The registry dropped its reference but other holders remain; their
    /// count is reported so the caller can wait or refuse to delete.
    StillReferenced { holders: usize },
}

struct Entry<H: ?Sized> {
    handle: Arc<H>,
    resident_bytes: u64,
    last_use: u64,
}

/// One in-progress cold open that concurrent acquires can wait on.
struct Flight<H: ?Sized> {
    outcome: Mutex<Option<Result<Arc<H>, FlightFailure>>>,
    ready: Condvar,
}

/// The opener's failure, kept as a code/message pair so every waiter can
/// receive an equivalent typed error without requiring `CoreError: Clone`.
#[derive(Clone, Debug)]
struct FlightFailure {
    rendered: String,
}

impl FlightFailure {
    fn into_error(self) -> CoreError {
        CoreError::Storage(format!(
            "snapshot registry: coalesced open failed: {}",
            self.rendered
        ))
    }
}

struct RegistryState<H: ?Sized> {
    resident: BTreeMap<SnapshotKey, Entry<H>>,
    in_flight: BTreeMap<SnapshotKey, Arc<Flight<H>>>,
    tick: u64,
    stats: SnapshotRegistryStats,
}

impl<H: ?Sized> RegistryState<H> {
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

    fn admit(
        &mut self,
        policy: SnapshotRegistryPolicy,
        key: SnapshotKey,
        opened: &OpenedSnapshot<H>,
    ) {
        if opened.resident_bytes > policy.max_resident_bytes {
            self.stats.oversize_uncached = self.stats.oversize_uncached.saturating_add(1);
            return;
        }
        self.evict_until_within(policy, opened.resident_bytes);
        self.tick = self.tick.saturating_add(1);
        let prior = self.resident.insert(
            key,
            Entry {
                handle: Arc::clone(&opened.handle),
                resident_bytes: opened.resident_bytes,
                last_use: self.tick,
            },
        );
        if let Some(prior) = prior {
            self.stats.resident_bytes = self
                .stats
                .resident_bytes
                .saturating_sub(prior.resident_bytes);
        }
        self.stats.resident_bytes = self
            .stats
            .resident_bytes
            .saturating_add(opened.resident_bytes);
        self.stats.entries = self.resident.len();
    }

    fn remove(&mut self, key: &SnapshotKey) -> Option<Arc<H>> {
        let removed = self.resident.remove(key)?;
        self.stats.resident_bytes = self
            .stats
            .resident_bytes
            .saturating_sub(removed.resident_bytes);
        self.stats.entries = self.resident.len();
        self.stats.invalidations = self.stats.invalidations.saturating_add(1);
        Some(removed.handle)
    }
}

/// Registry of opened sealed generations for one track.
pub struct SnapshotRegistry<H: ?Sized> {
    policy: SnapshotRegistryPolicy,
    state: Mutex<RegistryState<H>>,
}

impl<H: ?Sized + Send + Sync> SnapshotRegistry<H> {
    #[must_use]
    pub const fn new(policy: SnapshotRegistryPolicy) -> Self {
        Self {
            policy,
            state: Mutex::new(RegistryState {
                resident: BTreeMap::new(),
                in_flight: BTreeMap::new(),
                tick: 0,
                stats: SnapshotRegistryStats {
                    hits: 0,
                    misses: 0,
                    coalesced: 0,
                    evictions: 0,
                    oversize_uncached: 0,
                    invalidations: 0,
                    open_failures: 0,
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

    fn lock(&self) -> Result<std::sync::MutexGuard<'_, RegistryState<H>>, CoreError> {
        self.state
            .lock()
            .map_err(|err| CoreError::Storage(format!("snapshot registry poisoned: {err}")))
    }

    /// Return the handle for `key`, opening it with `open` if it is not
    /// resident and no other caller is already opening it.
    ///
    /// `open` runs outside the registry lock. Its success is retained under
    /// the policy; its failure is delivered to every caller that coalesced
    /// on this flight and is not retained, so the next acquire retries.
    pub fn acquire(
        &self,
        key: &SnapshotKey,
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
                return Self::await_flight(&flight).map(|handle| SnapshotAcquired {
                    handle,
                    outcome: SnapshotAcquireOutcome::Coalesced,
                });
            }
            state.stats.misses = state.stats.misses.saturating_add(1);
            let flight = Arc::new(Flight {
                outcome: Mutex::new(None),
                ready: Condvar::new(),
            });
            let _prior = state.in_flight.insert(key.clone(), Arc::clone(&flight));
            flight
        };

        let started = Instant::now();
        let opened = open();
        let elapsed = started.elapsed().as_nanos();

        // Settle the flight even if the registry lock is poisoned: a waiter
        // must never be left blocked on an outcome that will not arrive.
        let result = match self.lock() {
            Ok(mut state) => {
                let _removed = state.in_flight.remove(key);
                state.stats.cold_open_nanos = state.stats.cold_open_nanos.saturating_add(elapsed);
                match opened {
                    Ok(opened) => {
                        state.admit(self.policy, key.clone(), &opened);
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

        let published = match &result {
            Ok(handle) => Ok(Arc::clone(handle)),
            Err(err) => Err(FlightFailure {
                rendered: err.to_string(),
            }),
        };
        {
            let mut outcome = flight.outcome.lock().map_err(|err| {
                CoreError::Storage(format!("snapshot registry flight poisoned: {err}"))
            })?;
            *outcome = Some(published);
        }
        flight.ready.notify_all();
        result.map(|handle| SnapshotAcquired {
            handle,
            outcome: SnapshotAcquireOutcome::Miss {
                cold_open_nanos: elapsed,
            },
        })
    }

    fn await_flight(flight: &Flight<H>) -> Result<Arc<H>, CoreError> {
        let mut outcome = flight.outcome.lock().map_err(|err| {
            CoreError::Storage(format!("snapshot registry flight poisoned: {err}"))
        })?;
        loop {
            if let Some(settled) = outcome.as_ref() {
                return match settled {
                    Ok(handle) => Ok(Arc::clone(handle)),
                    Err(failure) => Err(failure.clone().into_error()),
                };
            }
            outcome = flight.ready.wait(outcome).map_err(|err| {
                CoreError::Storage(format!("snapshot registry flight poisoned: {err}"))
            })?;
        }
    }

    /// Drop the registry's reference to `key`, if resident. Handles held by
    /// in-flight queries stay alive. A flight in progress for the key is not
    /// affected: its result is admitted afresh when it lands, which is the
    /// correct outcome for a mutation that raced an open.
    pub fn invalidate(&self, key: &SnapshotKey) -> Result<bool, CoreError> {
        let mut state = self.lock()?;
        Ok(state.remove(key).is_some())
    }

    /// Drop the registry's reference to `key` and report whether anything
    /// else still holds the handle. Callers that intend to delete the
    /// generation's bytes must not proceed on `StillReferenced`.
    pub fn retire(&self, key: &SnapshotKey) -> Result<SnapshotRetireOutcome, CoreError> {
        let removed = {
            let mut state = self.lock()?;
            state.remove(key)
        };
        Ok(
            removed.map_or(SnapshotRetireOutcome::NotResident, |handle| {
                // Our own `handle` binding is one of the counted references.
                let holders = Arc::strong_count(&handle).saturating_sub(1);
                if holders == 0 {
                    SnapshotRetireOutcome::Released
                } else {
                    SnapshotRetireOutcome::StillReferenced { holders }
                }
            }),
        )
    }

    pub fn stats(&self) -> Result<SnapshotRegistryStats, CoreError> {
        Ok(self.lock()?.stats)
    }
}

/// The two per-track registries the search plane shares between its query
/// side (which acquires) and its ingest side (which invalidates).
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

    /// Drop both tracks' residency for one generation key. Used after any
    /// routed mutation that can change what an open handle would observe.
    pub fn invalidate(&self, key: &SnapshotKey) -> Result<(), CoreError> {
        let _lexical = self.lexical.invalidate(key)?;
        let _semantic = self.semantic.invalidate(key)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Arc, Barrier};
    use std::thread;
    use std::time::Duration;

    use quanta_index_contract::{ManifestGeneration, RepoId, RevisionId};
    use quanta_index_core::CoreError;

    use super::{
        OpenedSnapshot, SnapshotKey, SnapshotRegistry, SnapshotRegistryPolicy,
        SnapshotRetireOutcome,
    };

    type TestResult = Result<(), Box<dyn std::error::Error>>;

    #[derive(Debug)]
    struct Handle {
        key: SnapshotKey,
    }

    fn key(generation: u64) -> SnapshotKey {
        SnapshotKey::new(
            &RepoId::new("repo"),
            &RevisionId::new("rev"),
            ManifestGeneration::new(generation),
        )
    }

    fn policy(max_entries: usize, max_resident_bytes: u64) -> SnapshotRegistryPolicy {
        SnapshotRegistryPolicy::new(max_entries, max_resident_bytes)
            .expect("test policies are nonzero")
    }

    /// Acquire and keep only the handle; outcome-specific tests inspect the
    /// registry stats instead.
    fn get(
        registry: &SnapshotRegistry<Handle>,
        key: &SnapshotKey,
        open: impl FnOnce() -> Result<OpenedSnapshot<Handle>, CoreError>,
    ) -> Result<Arc<Handle>, CoreError> {
        registry.acquire(key, open).map(|acquired| acquired.handle)
    }

    fn opened(key: &SnapshotKey, bytes: u64) -> OpenedSnapshot<Handle> {
        OpenedSnapshot {
            handle: Arc::new(Handle { key: key.clone() }),
            resident_bytes: bytes,
        }
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

    /// A failed flight fails every waiter with a typed error and retains
    /// nothing, so the next acquire retries the opener.
    #[test]
    fn a_failed_open_is_shared_with_waiters_and_not_retained() -> TestResult {
        let registry = Arc::new(SnapshotRegistry::new(policy(4, 1_000)));
        let barrier = Arc::new(Barrier::new(2));
        let waiter = {
            let registry = Arc::clone(&registry);
            let barrier = Arc::clone(&barrier);
            thread::spawn(move || -> Result<Arc<Handle>, CoreError> {
                let _arrived = barrier.wait();
                thread::sleep(Duration::from_millis(10));
                get(&registry, &key(3), || Ok(opened(&key(3), 1)))
            })
        };
        let opener = get(&registry, &key(3), || {
            let _arrived = barrier.wait();
            thread::sleep(Duration::from_millis(50));
            Err(CoreError::NotFound("generation vanished".to_string()))
        });
        let waiter_outcome = waiter.join().map_err(|_panic| "waiter panicked")?;
        if opener.is_ok() {
            return Err("opener must fail".into());
        }
        match waiter_outcome {
            Ok(_) => {
                // The waiter may legitimately have arrived after the flight
                // settled and opened on its own; that is a miss, not a
                // coalesce, and the stats say which happened.
                let stats = registry.stats()?;
                if stats.coalesced != 0 {
                    return Err("a coalesced waiter must not succeed on a failed flight".into());
                }
            }
            Err(CoreError::Storage(message)) if message.contains("generation vanished") => {}
            Err(other) => return Err(format!("unexpected waiter error: {other}").into()),
        }
        if registry.stats()?.entries != 0 {
            return Err("a failed flight must not be retained".into());
        }
        let retried = get(&registry, &key(3), || Ok(opened(&key(3), 1)))?;
        if retried.key != key(3) {
            return Err("retry after failure did not open".into());
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
        if registry.invalidate(&key(2))? {
            return Err("key 2 should already have been evicted".into());
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
            SnapshotRetireOutcome::NotResident => {}
            reported @ (SnapshotRetireOutcome::Released
            | SnapshotRetireOutcome::StillReferenced { .. }) => {
                return Err(format!("evicted key reported {reported:?}").into());
            }
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
