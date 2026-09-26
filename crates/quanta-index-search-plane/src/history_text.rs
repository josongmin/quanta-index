//! The history text index as the search plane holds it (QI-BB-023
//! follow-up #1).
//!
//! The port the composition root wired, and the registry of opened epoch
//! handles that the query side acquires and the ingest side retires.
//!
//! An epoch's index is opened once and shared by every relevance query at
//! that epoch. The registry is what gates physical deletion: an epoch the
//! snapshot registry no longer retains is retired here first, and its
//! directory is discarded only when nothing else holds the handle — a
//! query mid-flight keeps its handle and the files it maps, and the next
//! mutation of the generation finds the epoch again and retries.
//!
//! A query takes a **claim** on its epoch under the ledger's read lock,
//! as part of the request's read view (`query_dispatcher/read_view`) — a
//! map lookup, no I/O — and lands it after the lock is released: the cold
//! open runs outside every lock (QI-BB-020 보완 #2), and concurrent claims
//! of the same epoch wait on the one open under their own budgets. The
//! claim is what keeps the prune-then-discard race closed: a mutation that
//! prunes the epoch takes the write lock only after the read released,
//! retires the epoch, finds the claim's open in flight or its handle held,
//! and defers the discard to the next pass over the generation.
//!
//! Residency is bounded by what the ledger retains: at most
//! `AUX_EPOCH_RETAIN + 1` epochs per history generation, and only the
//! generations retention keeps; a forgotten generation's handles are
//! retired with its rows. Weak custody also tracks detached handles until
//! their readers leave, without keeping the native indexes resident.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use quanta_index_contract::{AuxEpochV1, ManifestGeneration, RepoId, RevisionId};
use quanta_index_core::{
    AuxiliaryGenerationKeyV1, CoreError, HistoryTextDiscardOutcomeV1, HistoryTextIndexPort,
    HistoryTextSearcher, MetricPointV1, MetricSourcePort, RequestBudgetV1,
};

use crate::post_durable::defer_storage_failure;
use crate::single_flight::{AwaitFlightFailure, Flight, LiveHandleTracker};
use crate::snapshot_registry::SnapshotRetireOutcome;

/// The checkpoint name a reader's interruption carries while it waits on
/// another reader's cold open of the same epoch.
const AWAIT_OPEN_CHECKPOINT: &str = "history-text:await-open";

/// The port and the handle registry, shared by the query side and the
/// ingest side.
#[derive(Clone)]
pub struct HistoryTextIndexParts {
    pub port: Arc<dyn HistoryTextIndexPort + Send + Sync>,
    pub handles: Arc<HistoryTextHandles>,
    /// Discards that failed after the mutation letting their indexes go
    /// was durable; each is found again by the next pass.
    gc_failures: Arc<AtomicU64>,
}

impl HistoryTextIndexParts {
    /// Parts over `port` with a fresh registry.
    #[must_use]
    pub fn new(port: Arc<dyn HistoryTextIndexPort + Send + Sync>) -> Self {
        Self {
            port,
            handles: Arc::new(HistoryTextHandles::default()),
            gc_failures: Arc::new(AtomicU64::new(0)),
        }
    }

    /// Discard the epochs of `generation` that `retained` no longer names,
    /// after the mutation that let them go is durable (QI-BB-020).
    ///
    /// The mutation stands whatever the discard does: its receipt answers
    /// for rows that are durable and served, and failing it would tell the
    /// caller the opposite. A discard the storage failed is counted
    /// (`history_text_gc_failures_total`) and left where the next pass
    /// finds it: the next mutation of the generation reconciles its
    /// durable epochs again. A refusal is a finding about the index on
    /// disk, not a retry, and fails closed.
    pub fn reconcile_after_durable(
        &self,
        generation: &AuxiliaryGenerationKeyV1,
        retained: &[AuxEpochV1],
    ) -> Result<(), CoreError> {
        self.settle_gc(self.reconcile_generation(generation, retained))
    }

    /// Discard the indexes of every generation of the pair older than
    /// `below` that the ledger no longer knows (`known`), after the seal
    /// that forgot them is durable (QI-BB-020).
    ///
    /// Measured against the disk, not against the generations this pass
    /// forgot, so a generation whose discard failed on an earlier pass is
    /// found again here. A generation a reader still holds is deferred to
    /// the next pass; a failed listing or discard is settled like
    /// [`Self::reconcile_after_durable`]'s.
    pub fn sweep_forgotten_after_durable(
        &self,
        repo_id: &RepoId,
        revision_id: &RevisionId,
        below: ManifestGeneration,
        known: &BTreeSet<ManifestGeneration>,
    ) -> Result<(), CoreError> {
        let durable = match self.port.durable_generations(repo_id, revision_id) {
            Ok(durable) => durable,
            Err(error) => return self.settle_gc::<()>(Err(error)),
        };
        for generation in durable {
            if generation >= below || known.contains(&generation) {
                continue;
            }
            self.settle_gc(
                self.retire_and_discard_generation(&AuxiliaryGenerationKeyV1 {
                    repo_id: repo_id.clone(),
                    revision_id: revision_id.clone(),
                    generation,
                }),
            )?;
        }
        Ok(())
    }

    /// Count a GC step the storage failed and let it wait for the next
    /// pass; fail closed on a refusal.
    fn settle_gc<T>(&self, outcome: Result<T, CoreError>) -> Result<(), CoreError> {
        if let Err(error) = outcome {
            defer_storage_failure(error)?;
            let _prior = self.gc_failures.fetch_add(1, Ordering::Relaxed);
        }
        Ok(())
    }

    /// Claim the handle of `epoch`: a lookup, run under the ledger's read
    /// lock, that either finds the handle or reserves its open.
    pub(crate) fn claim(
        &self,
        generation: &AuxiliaryGenerationKeyV1,
        epoch: AuxEpochV1,
    ) -> Result<HistoryTextClaim, CoreError> {
        self.handles.claim(generation, epoch)
    }

    /// Land a claim outside every lock: open the epoch if the claim
    /// reserved its open, or wait under `budget` for the open another
    /// reader is running.
    pub(crate) fn land(
        &self,
        claim: HistoryTextClaim,
        budget: &RequestBudgetV1,
    ) -> Result<Arc<dyn HistoryTextSearcher>, CoreError> {
        self.handles.land(claim, budget, |generation, epoch| {
            self.port.open_epoch(generation, epoch)
        })
    }

    /// Retire the handle of `epoch` and, if nothing else holds it, discard
    /// its index. `None` when a holder remains: the discard is deferred.
    pub fn retire_and_discard_epoch(
        &self,
        generation: &AuxiliaryGenerationKeyV1,
        epoch: AuxEpochV1,
    ) -> Result<Option<HistoryTextDiscardOutcomeV1>, CoreError> {
        match self.handles.retire(generation, epoch)? {
            SnapshotRetireOutcome::NotResident | SnapshotRetireOutcome::Released => {
                Ok(Some(self.port.discard_epoch(generation, epoch)?))
            }
            SnapshotRetireOutcome::StillReferenced { .. } => Ok(None),
        }
    }

    /// Retire every handle of `generation` and, if nothing holds any of
    /// them, discard the generation's indexes. `None` when a holder
    /// remains: the next pass over the generation retries.
    pub fn retire_and_discard_generation(
        &self,
        generation: &AuxiliaryGenerationKeyV1,
    ) -> Result<Option<HistoryTextDiscardOutcomeV1>, CoreError> {
        let outcomes = self.handles.retire_generation(generation)?;
        if outcomes.iter().any(|(_epoch, outcome)| {
            matches!(outcome, SnapshotRetireOutcome::StillReferenced { .. })
        }) {
            return Ok(None);
        }
        Ok(Some(self.port.discard_generation(generation)?))
    }

    /// Discard every durable epoch of `generation` that `retained` does
    /// not name and no reader holds; the epochs still held are returned
    /// so the caller can say so.
    pub fn reconcile_generation(
        &self,
        generation: &AuxiliaryGenerationKeyV1,
        retained: &[AuxEpochV1],
    ) -> Result<Vec<AuxEpochV1>, CoreError> {
        let mut deferred = Vec::new();
        for epoch in self.port.durable_epochs(generation)? {
            if retained.contains(&epoch) {
                continue;
            }
            if self.retire_and_discard_epoch(generation, epoch)?.is_none() {
                deferred.push(epoch);
            }
        }
        Ok(deferred)
    }
}

/// `history_text_gc_failures_total`: discards that failed after the
/// mutation letting their indexes go was durable (QI-BB-020).
impl MetricSourcePort for HistoryTextIndexParts {
    fn scrape(&self) -> Result<Vec<MetricPointV1>, CoreError> {
        Ok(vec![MetricPointV1::counter(
            "history_text_gc_failures_total",
            self.gc_failures.load(Ordering::Relaxed),
        )])
    }
}

/// One epoch of one generation.
type EpochKey = (AuxiliaryGenerationKeyV1, AuxEpochV1);

/// One epoch's place in the registry: its opened handle, or the open a
/// claim is running for it.
enum Slot {
    Resident(Arc<dyn HistoryTextSearcher>),
    Opening(Arc<Flight<dyn HistoryTextSearcher>>),
}

/// A reader's hold on one epoch, taken under the ledger's read lock and
/// landed after it is released.
///
/// While a claim's open is in flight the epoch cannot be discarded:
/// retiring it defers the discard, exactly as a held handle does.
pub(crate) enum HistoryTextClaim {
    /// The handle was resident.
    Resident(Arc<dyn HistoryTextSearcher>),
    /// This reader opens the epoch.
    Open {
        generation: AuxiliaryGenerationKeyV1,
        epoch: AuxEpochV1,
        flight: Arc<Flight<dyn HistoryTextSearcher>>,
    },
    /// Another reader is opening it; this one waits on that open.
    Await(Arc<Flight<dyn HistoryTextSearcher>>),
}

/// Opened epoch handles, one per `(generation, epoch)`, and the opens in
/// flight.
#[derive(Default)]
pub struct HistoryTextHandles {
    state: Mutex<HistoryHandleState>,
}

#[derive(Default)]
struct HistoryHandleState {
    slots: BTreeMap<EpochKey, Slot>,
    tracked: LiveHandleTracker<EpochKey, dyn HistoryTextSearcher>,
}

impl HistoryHandleState {
    fn retire_key(&mut self, key: &EpochKey) -> SnapshotRetireOutcome {
        let removed = match self.slots.remove(key) {
            Some(Slot::Resident(handle)) => Some(handle),
            Some(Slot::Opening(flight)) => {
                let _previous = self.slots.insert(key.clone(), Slot::Opening(flight));
                let holders = self.tracked.external_holders(key, None).saturating_add(1);
                return SnapshotRetireOutcome::StillReferenced { holders };
            }
            None => None,
        };
        let holders = self.tracked.external_holders(key, removed.as_ref());
        if holders > 0 {
            SnapshotRetireOutcome::StillReferenced { holders }
        } else if removed.is_some() {
            SnapshotRetireOutcome::Released
        } else {
            SnapshotRetireOutcome::NotResident
        }
    }
}

impl HistoryTextHandles {
    fn lock(&self) -> Result<std::sync::MutexGuard<'_, HistoryHandleState>, CoreError> {
        self.state
            .lock()
            .map_err(|err| CoreError::Storage(format!("history text handles poisoned: {err}")))
    }

    /// Find the handle of `epoch`, join the open in flight for it, or
    /// reserve its open. No I/O: the caller holds the ledger's read lock.
    pub(crate) fn claim(
        &self,
        generation: &AuxiliaryGenerationKeyV1,
        epoch: AuxEpochV1,
    ) -> Result<HistoryTextClaim, CoreError> {
        let mut state = self.lock()?;
        let key = (generation.clone(), epoch);
        let claim = match state.slots.get(&key) {
            Some(Slot::Resident(handle)) => HistoryTextClaim::Resident(Arc::clone(handle)),
            Some(Slot::Opening(flight)) => HistoryTextClaim::Await(Arc::clone(flight)),
            None => {
                let flight = Arc::new(Flight::new());
                let _previous = state.slots.insert(key, Slot::Opening(Arc::clone(&flight)));
                HistoryTextClaim::Open {
                    generation: generation.clone(),
                    epoch,
                    flight,
                }
            }
        };
        drop(state);
        Ok(claim)
    }

    /// Land `claim`: run `open` for a reserved open, outside every lock,
    /// and make its handle resident; or wait under `budget` for another
    /// reader's open. A failed open is delivered to every waiter and not
    /// retained, so the next claim opens again.
    pub(crate) fn land(
        &self,
        claim: HistoryTextClaim,
        budget: &RequestBudgetV1,
        open: impl FnOnce(
            &AuxiliaryGenerationKeyV1,
            AuxEpochV1,
        ) -> Result<Box<dyn HistoryTextSearcher>, CoreError>,
    ) -> Result<Arc<dyn HistoryTextSearcher>, CoreError> {
        match claim {
            HistoryTextClaim::Resident(handle) => Ok(handle),
            HistoryTextClaim::Await(flight) => {
                Flight::await_outcome(&flight, budget, AWAIT_OPEN_CHECKPOINT).map_err(|failure| {
                    match failure {
                        AwaitFlightFailure::Flight(error)
                        | AwaitFlightFailure::Interrupted(error) => error,
                    }
                })
            }
            HistoryTextClaim::Open {
                generation,
                epoch,
                flight,
            } => {
                let (opened, opener_panic) = crate::single_flight::catch_open(|| {
                    open(&generation, epoch).map(Arc::<dyn HistoryTextSearcher>::from)
                });
                // Settle the flight even if the registry lock is poisoned:
                // a waiter must never be left blocked on an outcome that
                // will not arrive.
                let recorded = self.lock().map(|mut state| {
                    let key = (generation, epoch);
                    let ours = matches!(
                        state.slots.get(&key),
                        Some(Slot::Opening(pending)) if Arc::ptr_eq(pending, &flight)
                    );
                    if let Ok(handle) = &opened {
                        state.tracked.track(&key, handle);
                    }
                    if ours {
                        match &opened {
                            Ok(handle) => {
                                let _opening =
                                    state.slots.insert(key, Slot::Resident(Arc::clone(handle)));
                            }
                            Err(_failed) => {
                                let _opening = state.slots.remove(&key);
                            }
                        }
                    }
                });
                let outcome = recorded.and(opened);
                let settled = flight.settle(outcome.clone());
                if let Some(payload) = opener_panic {
                    std::panic::resume_unwind(payload);
                }
                settled?;
                outcome
            }
        }
    }

    /// Drop the registry's reference to `epoch` and report whether
    /// anything else still holds it, including on repeated retirement.
    /// An open in flight also defers the discard.
    pub fn retire(
        &self,
        generation: &AuxiliaryGenerationKeyV1,
        epoch: AuxEpochV1,
    ) -> Result<SnapshotRetireOutcome, CoreError> {
        let mut state = self.lock()?;
        let key = (generation.clone(), epoch);
        let outcome = state.retire_key(&key);
        drop(state);
        Ok(outcome)
    }

    /// Retire every epoch of `generation`.
    pub fn retire_generation(
        &self,
        generation: &AuxiliaryGenerationKeyV1,
    ) -> Result<Vec<(AuxEpochV1, SnapshotRetireOutcome)>, CoreError> {
        let mut state = self.lock()?;
        let keys = state
            .slots
            .keys()
            .chain(state.tracked.keys())
            .filter(|(key, _epoch)| key == generation)
            .cloned()
            .collect::<BTreeSet<_>>();
        let mut outcomes = Vec::new();
        for key in keys {
            outcomes.push((key.1, state.retire_key(&key)));
        }
        drop(state);
        Ok(outcomes)
    }
}

#[cfg(test)]
#[expect(
    clippy::panic_in_result_fn,
    reason = "Result-returning registry tests assert with `assert!` on the registry's own answers"
)]
mod tests {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::mpsc;

    use quanta_index_contract::{AuxEpochV1, ManifestGeneration, RepoId, RevisionId};
    use quanta_index_core::{
        AuxiliaryGenerationKeyV1, CoreError, HistoryTextBuildV1, HistoryTextDiscardOutcomeV1,
        HistoryTextIndexPort, HistoryTextSearcher, RequestBudgetV1,
    };

    use super::{HistoryTextClaim, HistoryTextHandles, HistoryTextIndexParts};
    use crate::query_dispatcher::tests::support::history_text::MemoryHistoryTextIndex;

    type TestResult = Result<(), Box<dyn std::error::Error>>;

    fn generation() -> AuxiliaryGenerationKeyV1 {
        AuxiliaryGenerationKeyV1 {
            repo_id: RepoId::new("repo-history-text")
                .expect("static fixture ID satisfies canonical policy"),
            revision_id: RevisionId::new("rev-history-text")
                .expect("static fixture ID satisfies canonical policy"),
            generation: ManifestGeneration::new(3),
        }
    }

    const EPOCH: AuxEpochV1 = AuxEpochV1::new(1);

    fn index_with(epochs: &[u64]) -> Result<Arc<MemoryHistoryTextIndex>, CoreError> {
        let index = MemoryHistoryTextIndex::shared();
        for epoch in epochs {
            let _receipt = index.publish_epoch(
                &generation(),
                AuxEpochV1::new(*epoch),
                HistoryTextBuildV1::Full { docs: Vec::new() },
            )?;
        }
        Ok(index)
    }

    fn never_opened(
        _generation: &AuxiliaryGenerationKeyV1,
        _epoch: AuxEpochV1,
    ) -> Result<Box<dyn HistoryTextSearcher>, CoreError> {
        Err(CoreError::Storage(
            "a claim that was not an open must not open".to_string(),
        ))
    }

    /// A claim is a lookup and opens nothing; its open runs when it lands,
    /// under no registry lock, and a claim of the same epoch meanwhile waits
    /// on that one open and shares its handle (QI-BB-020).
    ///
    /// The opener's open is parked until the test has claimed and landed
    /// another epoch — which would block forever if the registry held a
    /// lock across an open.
    #[test]
    fn a_claim_opens_nothing_and_a_parked_open_blocks_no_other_epoch() -> TestResult {
        let index = index_with(&[1, 2])?;
        let handles = HistoryTextHandles::default();
        let opens = AtomicUsize::new(0);
        let unbounded = RequestBudgetV1::unbounded();
        let first = handles.claim(&generation(), EPOCH)?;
        let second = handles.claim(&generation(), EPOCH)?;
        assert!(matches!(first, HistoryTextClaim::Open { .. }));
        assert!(matches!(second, HistoryTextClaim::Await(_)));
        assert_eq!(opens.load(Ordering::SeqCst), 0, "a claim opens nothing");

        let (started_tx, started_rx) = mpsc::channel::<()>();
        let (release_tx, release_rx) = mpsc::channel::<()>();
        let (opened, other) =
            std::thread::scope(|scope| -> Result<_, Box<dyn std::error::Error>> {
                let (registry, budget, counter, source) = (&handles, &unbounded, &opens, &index);
                let opener = scope.spawn(move || {
                    registry.land(first, budget, |generation, epoch| {
                        let _prior = counter.fetch_add(1, Ordering::SeqCst);
                        started_tx
                            .send(())
                            .map_err(|err| CoreError::Storage(err.to_string()))?;
                        release_rx
                            .recv()
                            .map_err(|err| CoreError::Storage(err.to_string()))?;
                        source.open_epoch(generation, epoch)
                    })
                });
                started_rx.recv()?;
                let other = handles.claim(&generation(), AuxEpochV1::new(2))?;
                let other = handles.land(other, &unbounded, |generation, epoch| {
                    index.open_epoch(generation, epoch)
                })?;
                release_tx.send(())?;
                let landed = opener
                    .join()
                    .map_err(|_panic| "the opener thread panicked")??;
                Ok((landed, other))
            })?;
        let shared = handles.land(second, &unbounded, never_opened)?;
        assert!(
            Arc::ptr_eq(&opened, &shared),
            "the waiter shares the one open"
        );
        assert!(!Arc::ptr_eq(&opened, &other));
        assert_eq!(opens.load(Ordering::SeqCst), 1, "epoch 1 opened once");
        match handles.claim(&generation(), EPOCH)? {
            HistoryTextClaim::Resident(handle) if Arc::ptr_eq(&handle, &opened) => Ok(()),
            HistoryTextClaim::Resident(_)
            | HistoryTextClaim::Open { .. }
            | HistoryTextClaim::Await(_) => Err("the landed handle is resident".into()),
        }
    }

    #[test]
    fn panicked_epoch_opener_releases_the_slot_for_a_later_claim() -> TestResult {
        let index = index_with(&[1])?;
        let handles = HistoryTextHandles::default();
        let budget = RequestBudgetV1::for_duration(std::time::Duration::from_millis(50));
        let first = handles.claim(&generation(), EPOCH)?;
        let waiting = handles.claim(&generation(), EPOCH)?;
        let panicked = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _opened = handles.land(first, &budget, |_generation, _epoch| {
                panic!("injected history epoch open panic");
            });
        }));
        if panicked.is_ok() {
            return Err("the injected history opener panic must propagate".into());
        }
        if !matches!(
            handles.land(waiting, &budget, never_opened),
            Err(CoreError::Storage(message)) if message.contains("opener panicked")
        ) {
            return Err("the panicked opener did not settle its existing waiter".into());
        }
        let next = handles.claim(&generation(), EPOCH)?;
        if !matches!(next, HistoryTextClaim::Open { .. }) {
            return Err("a panicked history open left its slot reserved".into());
        }
        let _opened = handles.land(next, &budget, |generation, epoch| {
            index.open_epoch(generation, epoch)
        })?;
        Ok(())
    }

    /// An epoch retired while a claim's open is in flight is not
    /// discarded: the discard is deferred, as for a held handle, and goes
    /// through once nothing holds the epoch (QI-BB-020).
    #[test]
    fn retiring_an_epoch_whose_open_is_in_flight_defers_its_discard() -> TestResult {
        let index = index_with(&[1])?;
        let parts = HistoryTextIndexParts::new(index.clone());
        let unbounded = RequestBudgetV1::unbounded();
        let claim = parts.claim(&generation(), EPOCH)?;
        assert_eq!(parts.retire_and_discard_epoch(&generation(), EPOCH)?, None);
        assert_eq!(
            index.epochs_of(&generation())?,
            vec![EPOCH],
            "nothing was discarded"
        );
        let handle = parts.land(claim, &unbounded)?;
        assert_eq!(
            parts.retire_and_discard_epoch(&generation(), EPOCH)?,
            None,
            "the reader still holds the landed handle"
        );
        assert_eq!(
            parts.retire_and_discard_epoch(&generation(), EPOCH)?,
            None,
            "a retry must not discard after the resident slot was removed"
        );
        drop(handle);
        match parts.retire_and_discard_epoch(&generation(), EPOCH)? {
            Some(HistoryTextDiscardOutcomeV1::Discarded { .. }) => {}
            other => return Err(format!("released, the epoch is discarded: {other:?}").into()),
        }
        assert!(index.epochs_of(&generation())?.is_empty());
        Ok(())
    }

    #[test]
    fn generation_discard_retries_keep_live_retired_epoch_pinned() -> TestResult {
        let index = index_with(&[1])?;
        let parts = HistoryTextIndexParts::new(index.clone());
        let claim = parts.claim(&generation(), EPOCH)?;
        let handle = parts.land(claim, &RequestBudgetV1::unbounded())?;
        for _retry in 0..2 {
            if parts
                .retire_and_discard_generation(&generation())?
                .is_some()
            {
                return Err("generation discard ignored a live retired epoch".into());
            }
            if index.epochs_of(&generation())? != vec![EPOCH] {
                return Err("generation bytes were discarded under a live epoch handle".into());
            }
        }
        drop(handle);
        if !matches!(
            parts.retire_and_discard_generation(&generation())?,
            Some(HistoryTextDiscardOutcomeV1::Discarded { .. })
        ) {
            return Err("generation discard did not resume after the handle dropped".into());
        }
        Ok(())
    }

    /// A failed open reaches every waiter by value and is not retained:
    /// the next claim opens again.
    #[test]
    fn a_failed_open_reaches_every_waiter_and_is_not_retained() -> TestResult {
        let handles = HistoryTextHandles::default();
        let unbounded = RequestBudgetV1::unbounded();
        let first = handles.claim(&generation(), EPOCH)?;
        let second = handles.claim(&generation(), EPOCH)?;
        let failure = || CoreError::Typed {
            code: quanta_index_contract::SearchPlaneErrorCodeV2::HistoryTextIndexNotReady,
            message: "injected".to_string(),
        };
        for landed in [
            handles.land(first, &unbounded, |_generation, _epoch| Err(failure())),
            handles.land(second, &unbounded, never_opened),
        ] {
            match landed {
                Err(CoreError::Typed {
                    code: quanta_index_contract::SearchPlaneErrorCodeV2::HistoryTextIndexNotReady,
                    ..
                }) => {}
                Err(other) => {
                    return Err(format!("the opener's failure by value: {other:?}").into());
                }
                Ok(_handle) => return Err("a failed open served a handle".into()),
            }
        }
        assert!(matches!(
            handles.claim(&generation(), EPOCH)?,
            HistoryTextClaim::Open { .. }
        ));
        Ok(())
    }

    /// A reader waiting on another's open waits under its own budget: an
    /// expired budget ends the wait with the typed interruption while the
    /// open is still out.
    #[test]
    fn a_waiter_under_an_expired_budget_is_interrupted() -> TestResult {
        let handles = HistoryTextHandles::default();
        let _opener = handles.claim(&generation(), EPOCH)?;
        let waiter = handles.claim(&generation(), EPOCH)?;
        let expired = RequestBudgetV1::until(std::time::Instant::now());
        match handles.land(waiter, &expired, never_opened) {
            Err(CoreError::Typed { code, message })
                if message.contains("history-text:await-open") =>
            {
                assert_eq!(
                    code,
                    quanta_index_contract::SearchPlaneErrorCodeV2::RequestDeadlineExceeded
                );
                Ok(())
            }
            Err(other) => Err(format!("the waiter's own interruption: {other:?}").into()),
            Ok(_handle) => Err("an expired waiter served a handle".into()),
        }
    }
}
