//! Bounded, synchronous group commit of independent live observations.
//!
//! Only concurrent observations share a transaction. A caller never proceeds
//! using an uncommitted clock sample or a cached live-ticket decision.
//! Queue admission waits use the original request deadline and cancellation.

use std::sync::{
    Mutex, MutexGuard, OnceLock, TryLockError as MutexTryLockError,
    atomic::{AtomicBool, Ordering},
};

use quanta_index_core::index::{CatalogReadAttempt, MemoryLease, MemoryLifetime, WorkLimits};

use super::*;

const MAX_OBSERVATIONS: usize = 64;
const COALESCE: Duration = Duration::from_micros(250);
const POLL: Duration = Duration::from_millis(1);

struct Observation {
    catalog: usize,
    ticket: ReadTicketLease,
    inode: (Vec<u8>, Vec<u8>),
    // Only checkpoint authority is copied. No work or memory grant is created.
    budget: WorkBudget,
    result: OnceLock<Result<(), IndexError>>,
    waiter: std::thread::Thread,
    _memory: Option<MemoryLease>,
}

struct Pending {
    entries: [Option<Arc<Observation>>; MAX_OBSERVATIONS],
}

impl Default for Pending {
    fn default() -> Self {
        Self {
            entries: std::array::from_fn(|_| None),
        }
    }
}

#[derive(Default)]
pub(super) struct ObservationGroup {
    pending: Mutex<Pending>,
    active: AtomicBool,
    _memory: Option<MemoryLease>,
}

fn poisoned() -> IndexError {
    IndexError::Storage("read observation group poisoned".into())
}

fn lock_for_request<'a, T>(
    mutex: &'a Mutex<T>,
    budget: &WorkBudget,
) -> Result<MutexGuard<'a, T>, IndexError> {
    loop {
        budget.checkpoint()?;
        match mutex.try_lock() {
            Ok(guard) => {
                budget.checkpoint()?;
                return Ok(guard);
            }
            Err(MutexTryLockError::WouldBlock) => std::thread::park_timeout(POLL),
            Err(MutexTryLockError::Poisoned(_error)) => return Err(poisoned()),
        }
    }
}

impl Observation {
    fn complete(&self, result: Result<(), IndexError>) -> Result<(), IndexError> {
        self.result.set(result).map_err(|_duplicate| {
            IndexError::BackendViolation("read observation completed twice".into())
        })?;
        self.waiter.unpark();
        Ok(())
    }

    fn completed_result(&self, budget: &WorkBudget) -> Option<Result<(), IndexError>> {
        self.result.get().map(|result| match result {
            Ok(()) => budget.checkpoint(),
            Err(error) => {
                let _observed = budget.checkpoint();
                Err(error.clone())
            }
        })
    }
}

// Cleanup must release leadership even when the original request is already
// interrupted. It cannot wait for another request to release a metadata mutex.
struct Leadership<'a>(&'a AtomicBool);

impl Drop for Leadership<'_> {
    fn drop(&mut self) {
        self.0.store(false, Ordering::Release);
    }
}

// Only queued entries are retired here, under the queue mutex. A live leader
// can publish its acquisition failure without that mutex, so only an inactive
// queue may additionally complete interrupted, not-yet-taken entries.
fn retire_finished(
    pending: &mut Pending,
    inactive: bool,
) -> Result<[Option<Arc<Observation>>; MAX_OBSERVATIONS], IndexError> {
    let mut retired = std::array::from_fn(|_| None);
    for (slot, destination) in pending.entries.iter_mut().zip(&mut retired) {
        if let Some(entry) = slot.as_ref() {
            if inactive
                && entry.result.get().is_none()
                && let Err(error) = entry.budget.checkpoint()
            {
                entry.complete(Err(error))?;
            }
            if entry.result.get().is_some() {
                *destination = slot.take();
            }
        }
    }
    Ok(retired)
}

impl ObservationGroup {
    fn retire_for_request(&self, budget: &WorkBudget) -> Result<(), IndexError> {
        let retired = {
            let mut pending = lock_for_request(&self.pending, budget)?;
            retire_finished(&mut pending, !self.active.load(Ordering::Acquire))?
        };
        drop(retired);
        budget.checkpoint()
    }

    fn complete_failed_leader(
        &self,
        leader: &Arc<Observation>,
        error: IndexError,
    ) -> Result<(), IndexError> {
        let published = leader.complete(Err(error));
        // This only retires response data, never the retained pin. If another
        // request owns the mutex, a later bounded retirement will reclaim it.
        if let Ok(mut pending) = self.pending.try_lock() {
            for slot in &mut pending.entries {
                if slot
                    .as_ref()
                    .is_some_and(|entry| Arc::ptr_eq(entry, leader))
                {
                    *slot = None;
                    break;
                }
            }
        }
        published
    }

    fn observe(
        &self,
        catalog: &SqliteCatalog,
        ticket: ReadTicketLease,
        inode: (Vec<u8>, Vec<u8>),
        budget: &WorkBudget,
    ) -> Result<(), IndexError> {
        budget.checkpoint()?;
        // Reclaim ended requests before asking the original memory authority
        // for another retained entry; a full grant must not prevent cleanup.
        self.retire_for_request(budget)?;
        let bytes = std::mem::size_of::<Observation>()
            .checked_add(std::mem::size_of::<[usize; 2]>())
            .and_then(|bytes| bytes.checked_add(16))
            .ok_or(IndexError::ResourceLimited)?;
        let bytes = u64::try_from(bytes).map_err(|_error| IndexError::ResourceLimited)?;
        // Cancellation can return while an elected leader is finishing the
        // batch. Fund that short escaping lifetime without retaining execution.
        let memory = budget.reserve_memory(bytes, MemoryLifetime::Retained)?;
        let observation = Arc::new(Observation {
            catalog: std::ptr::from_ref(catalog).addr(),
            ticket,
            inode,
            budget: WorkBudget::with_request(
                WorkLimits {
                    reads: 0,
                    writes: 0,
                    bytes: 0,
                    mutations: 0,
                },
                budget.request().clone(),
            ),
            result: OnceLock::new(),
            waiter: std::thread::current(),
            _memory: memory,
        });
        let mut queued = false;
        loop {
            if let Some(result) = observation.completed_result(budget) {
                return result;
            }
            let mut pending = match lock_for_request(&self.pending, budget) {
                Ok(pending) => pending,
                Err(error) => {
                    // A committed result may have arrived as this wait ended.
                    // Preserve a published failure before reporting interruption.
                    return observation.completed_result(budget).unwrap_or(Err(error));
                }
            };
            if !queued {
                if let Some(slot) = pending.entries.iter_mut().find(|entry| entry.is_none()) {
                    *slot = Some(Arc::clone(&observation));
                    queued = true;
                } else {
                    let retired =
                        retire_finished(&mut pending, !self.active.load(Ordering::Acquire))?;
                    if retired.iter().any(Option::is_some) {
                        drop(pending);
                        // Last retained charges may be released here, outside
                        // the queue mutex and before electing another leader.
                        drop(retired);
                        continue;
                    }
                }
            }
            if self.active.load(Ordering::Acquire) {
                drop(pending);
                std::thread::park_timeout(POLL);
            } else {
                // Acquire pairs with leadership release after all results are
                // published. Recheck before electing a leader for this entry.
                if observation.result.get().is_some() {
                    drop(pending);
                    continue;
                }
                self.active.store(true, Ordering::Release);
                drop(pending);
                self.lead(catalog, &observation)?;
                if !queued {
                    // A full queue can be helped before admitting this request.
                    // The helper's own ticket must still be observed afterwards.
                    std::thread::park_timeout(POLL);
                }
            }
        }
    }

    fn lead(&self, catalog: &SqliteCatalog, leader: &Arc<Observation>) -> Result<(), IndexError> {
        let _leadership = Leadership(&self.active);
        std::thread::sleep(COALESCE);
        let mut connection =
            match observe_ticket_validation(catalog, Some(&leader.budget), 201, || {
                catalog.lock_native_budget(&leader.budget)
            }) {
                Ok(connection) => connection,
                Err(error) => return self.complete_failed_leader(leader, error),
            };
        let mut batch: [Option<Arc<Observation>>; MAX_OBSERVATIONS] = std::array::from_fn(|_| None);
        {
            // A leader must finish settling its owned batch after interruption.
            // Its caller's result and leadership release never reacquire this
            // mutex after the connection has been released.
            let mut pending = match lock_for_request(&self.pending, &leader.budget) {
                Ok(pending) => pending,
                Err(error) => {
                    drop(connection);
                    return self.complete_failed_leader(leader, error);
                }
            };
            for (entry, destination) in pending.entries.iter_mut().zip(&mut batch) {
                if entry
                    .as_ref()
                    .is_some_and(|entry| entry.catalog == leader.catalog)
                {
                    *destination = entry.take();
                }
            }
        }
        if batch.iter().all(Option::is_none) {
            drop(connection);
            return Ok(());
        }
        let mut results: [Option<Result<u64, IndexError>>; MAX_OBSERVATIONS] =
            std::array::from_fn(|_| None);
        let committed = commit_batch(
            catalog,
            &mut connection,
            &batch,
            &mut results,
            &leader.budget,
        );
        // No reader is awakened while a connection is still held.
        drop(connection);
        let mut publication = Ok(());
        for (entry, result) in batch.iter().zip(results) {
            if let Some(entry) = entry {
                // An acquisition failure may already have completed a queued
                // request before another leader takes its leftover queue entry.
                if entry.result.get().is_some() {
                    continue;
                }
                let result = match result {
                    Some(Err(error)) => Err(error),
                    Some(Ok(_sample)) => committed.clone(),
                    None => match &committed {
                        Err(error) => Err(error.clone()),
                        Ok(()) => Err(IndexError::BackendViolation(
                            "read observation missing batch result".into(),
                        )),
                    },
                };
                let completed = entry.complete(result);
                if publication.is_ok() {
                    publication = completed;
                }
            }
        }
        publication
    }
}

fn commit_batch(
    catalog: &SqliteCatalog,
    connection: &mut Connection,
    batch: &[Option<Arc<Observation>>; MAX_OBSERVATIONS],
    results: &mut [Option<Result<u64, IndexError>>; MAX_OBSERVATIONS],
    leader_budget: &WorkBudget,
) -> Result<(), IndexError> {
    let tx = observe_ticket_validation(catalog, Some(leader_budget), 202, || begin(connection))?;
    let mut prior_sample = None;
    for (entry, result) in batch.iter().zip(results.iter_mut()) {
        if let Some(entry) = entry {
            if entry.result.get().is_some() {
                continue;
            }
            *result = Some((|| {
                entry.budget.checkpoint()?;
                let pin = observe_ticket_validation(catalog, Some(&entry.budget), 203, || {
                    let session = checked_session(&tx, entry.ticket.session)?;
                    if (session.2, session.3) != entry.inode {
                        return Err(IndexError::Storage("read session inode mismatch".into()));
                    }
                    checked_pin(&tx, entry.ticket)
                })?;
                let now = observe_ticket_validation(catalog, Some(&entry.budget), 204, || {
                    observed_catalog_now_ms(&tx, catalog.clock.now_unix_ms())
                })?;
                // Each successful observation has an ordered durable floor,
                // including regression within this not-yet-committed group.
                if prior_sample.is_some_and(|prior| now < prior) {
                    return Err(IndexError::Storage("catalog clock regressed".into()));
                }
                if now >= uint(pin.4)? {
                    return Err(IndexError::SnapshotUnavailable);
                }
                entry.budget.checkpoint()?;
                prior_sample = Some(now);
                Ok(now)
            })());
        }
    }
    let mut maximum = None;
    for (entry, result) in batch.iter().zip(results.iter_mut()) {
        if let (Some(entry), Some(Ok(sample))) = (entry, result.as_ref()) {
            match entry.budget.checkpoint() {
                Ok(()) => maximum = Some(maximum.map_or(*sample, |prior: u64| prior.max(*sample))),
                Err(error) => *result = Some(Err(error)),
            }
        }
    }
    if let Some(maximum) = maximum {
        // The whole batch's validated maximum is persisted once. A later
        // failure cannot erase any successful member's intermediate floor.
        let _durable_sample = catalog_now_ms(&tx, maximum)?;
        observe_ticket_validation(catalog, Some(leader_budget), 205, || {
            tx.commit().map_err(|error| sql(&error))
        })
    } else {
        // No authority is returned and no clock observation must be persisted.
        drop(tx);
        Ok(())
    }
}

struct ReadAttempt<'a> {
    catalog: &'a SqliteCatalog,
    session: &'a dyn ReadSessionLease,
    ticket: ReadTicketLease,
    budget: &'a WorkBudget,
    failed: Option<IndexError>,
    group: Arc<ObservationGroup>,
    _memory: Option<MemoryLease>,
}

impl CatalogReadAttempt for ReadAttempt<'_> {
    fn ticket(&self) -> ReadTicketLease {
        self.ticket
    }

    fn observe(&mut self) -> Result<(), IndexError> {
        if let Some(error) = &self.failed {
            return Err(error.clone());
        }
        let result = (|| {
            let identity = observe_ticket_validation(self.catalog, Some(self.budget), 200, || {
                guard(self.catalog, self.session)
            })?;
            if identity.binding != self.ticket.session {
                return Err(IndexError::SnapshotUnavailable);
            }
            self.group
                .observe(self.catalog, self.ticket, identity.inode, self.budget)
        })();
        if let Err(error) = &result {
            self.failed = Some(error.clone());
        }
        result
    }

    fn finalize(mut self: Box<Self>) -> Result<(), IndexError> {
        self.observe()
    }

    fn abort(self: Box<Self>) {}
}

pub(super) fn start<'a>(
    catalog: &'a SqliteCatalog,
    session: &'a dyn ReadSessionLease,
    ticket: ReadTicketLease,
    budget: &'a WorkBudget,
) -> Result<Box<dyn CatalogReadAttempt + 'a>, IndexError> {
    // Preserve entry refusal precedence: authenticate the original OS guard
    // and session binding before testing request interruption or allocating.
    let identity =
        observe_ticket_validation(catalog, Some(budget), 200, || guard(catalog, session))?;
    if identity.binding != ticket.session {
        return Err(IndexError::SnapshotUnavailable);
    }
    let group = {
        // Initialization includes funding through the caller's memory port.
        // Other original requests must remain interruptible while it waits.
        let mut slot = lock_for_request(&identity.lease.read_observations, budget)?;
        let group = if let Some(group) = slot.as_ref() {
            Arc::clone(group)
        } else {
            let bytes = std::mem::size_of::<ObservationGroup>()
                .checked_add(std::mem::size_of::<[usize; 2]>())
                .ok_or(IndexError::ResourceLimited)?;
            let bytes = u64::try_from(bytes).map_err(|_error| IndexError::ResourceLimited)?;
            let memory = budget.reserve_memory(bytes, MemoryLifetime::Retained)?;
            let group = Arc::new(ObservationGroup {
                _memory: memory,
                ..ObservationGroup::default()
            });
            *slot = Some(Arc::clone(&group));
            group
        };
        drop(slot);
        group
    };
    group.retire_for_request(budget)?;
    let memory = budget.reserve_memory(
        u64::try_from(std::mem::size_of::<ReadAttempt<'_>>())
            .map_err(|_error| IndexError::ResourceLimited)?,
        MemoryLifetime::Temporary,
    )?;
    let attempt = Box::new(ReadAttempt {
        catalog,
        session,
        ticket,
        budget,
        failed: None,
        group,
        _memory: memory,
    });
    attempt
        .group
        .observe(catalog, ticket, identity.inode, budget)?;
    Ok(attempt)
}

#[cfg(test)]
mod tests;
