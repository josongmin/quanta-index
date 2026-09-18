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
//! The query side acquires under the ledger's read lock, as part of the
//! request's read view (`query_dispatcher/read_view.rs`): a mutation that
//! prunes the epoch takes the write lock after the read released, retires
//! the handle, sees the query's hold, and defers. Acquiring outside the
//! lock could let the prune and the discard run between a read that found
//! the epoch retained and the open.
//!
//! Residency is bounded by what the ledger retains: at most
//! `AUX_EPOCH_RETAIN + 1` epochs per history generation, and only the
//! generations retention keeps; a forgotten generation's handles are
//! retired with its rows.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use quanta_index_contract::AuxEpochV1;
use quanta_index_core::{
    AuxiliaryGenerationKeyV1, CoreError, HistoryTextDiscardOutcomeV1, HistoryTextIndexPort,
    HistoryTextSearcher,
};

use crate::snapshot_registry::SnapshotRetireOutcome;

/// The port and the handle registry, shared by the query side and the
/// ingest side.
#[derive(Clone)]
pub struct HistoryTextIndexParts {
    pub port: Arc<dyn HistoryTextIndexPort + Send + Sync>,
    pub handles: Arc<HistoryTextHandles>,
}

impl HistoryTextIndexParts {
    /// Parts over `port` with a fresh registry.
    #[must_use]
    pub fn new(port: Arc<dyn HistoryTextIndexPort + Send + Sync>) -> Self {
        Self {
            port,
            handles: Arc::new(HistoryTextHandles::default()),
        }
    }

    /// Open (or share) the handle of `epoch`.
    pub fn acquire(
        &self,
        generation: &AuxiliaryGenerationKeyV1,
        epoch: AuxEpochV1,
    ) -> Result<Arc<dyn HistoryTextSearcher>, CoreError> {
        self.handles.acquire(generation, epoch, || {
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

/// The resident handles, keyed by generation and epoch.
type ResidentHandles =
    BTreeMap<(AuxiliaryGenerationKeyV1, AuxEpochV1), Arc<dyn HistoryTextSearcher>>;

/// Opened epoch handles, one per `(generation, epoch)`.
#[derive(Default)]
pub struct HistoryTextHandles {
    resident: Mutex<ResidentHandles>,
}

impl HistoryTextHandles {
    fn lock(&self) -> Result<std::sync::MutexGuard<'_, ResidentHandles>, CoreError> {
        self.resident
            .lock()
            .map_err(|err| CoreError::Storage(format!("history text handles poisoned: {err}")))
    }

    /// The handle for `epoch`, opened with `open` if it is not resident.
    ///
    /// The open runs under the registry lock: an epoch is opened once per
    /// process lifetime and the open is a bounded file map, so serializing
    /// cold opens is cheaper than a flight table and keeps a concurrent
    /// acquire from opening the same epoch twice.
    pub fn acquire(
        &self,
        generation: &AuxiliaryGenerationKeyV1,
        epoch: AuxEpochV1,
        open: impl FnOnce() -> Result<Box<dyn HistoryTextSearcher>, CoreError>,
    ) -> Result<Arc<dyn HistoryTextSearcher>, CoreError> {
        let mut resident = self.lock()?;
        let key = (generation.clone(), epoch);
        if let Some(handle) = resident.get(&key) {
            let handle = Arc::clone(handle);
            drop(resident);
            return Ok(handle);
        }
        let handle: Arc<dyn HistoryTextSearcher> = Arc::from(open()?);
        let _previous = resident.insert(key, Arc::clone(&handle));
        drop(resident);
        Ok(handle)
    }

    /// Drop the registry's reference to `epoch` and report whether
    /// anything else still holds it.
    pub fn retire(
        &self,
        generation: &AuxiliaryGenerationKeyV1,
        epoch: AuxEpochV1,
    ) -> Result<SnapshotRetireOutcome, CoreError> {
        let removed = self.lock()?.remove(&(generation.clone(), epoch));
        Ok(Self::outcome(removed))
    }

    /// Retire every epoch of `generation`.
    pub fn retire_generation(
        &self,
        generation: &AuxiliaryGenerationKeyV1,
    ) -> Result<Vec<(AuxEpochV1, SnapshotRetireOutcome)>, CoreError> {
        let mut resident = self.lock()?;
        let epochs: Vec<AuxEpochV1> = resident
            .keys()
            .filter(|(key, _epoch)| key == generation)
            .map(|(_key, epoch)| *epoch)
            .collect();
        let mut outcomes = Vec::with_capacity(epochs.len());
        for epoch in epochs {
            let removed = resident.remove(&(generation.clone(), epoch));
            outcomes.push((epoch, Self::outcome(removed)));
        }
        drop(resident);
        Ok(outcomes)
    }

    fn outcome(removed: Option<Arc<dyn HistoryTextSearcher>>) -> SnapshotRetireOutcome {
        removed.map_or(SnapshotRetireOutcome::NotResident, |handle| {
            // Our own `handle` binding is one of the counted references.
            let holders = Arc::strong_count(&handle).saturating_sub(1);
            if holders == 0 {
                SnapshotRetireOutcome::Released
            } else {
                SnapshotRetireOutcome::StillReferenced { holders }
            }
        })
    }
}
