//! Bounded daemon custody for explicit Native read sessions.
//!
//! The Catalog ticket is the read authority. This registry only binds its
//! lifetime to an authenticated uid and an unguessable, replayable handle.

use std::{
    collections::BTreeMap,
    fs::File,
    io::Read as _,
    ops::Bound::{Excluded, Unbounded},
    sync::{
        Arc, Mutex, MutexGuard, TryLockError,
        atomic::{AtomicUsize, Ordering},
    },
    thread,
    time::{Duration, Instant},
};

use quanta_index_contract::{
    index::ViewSelection,
    native_protocol::{NativeReadSessionId, NativeReadSessionState, NativeReadSessionStatus},
};
use quanta_index_core::index::{IndexError, MemoryLease, WorkBudget};
use quanta_index_kernel::ReadTicket;

const SLOT_ALLOWANCE_BYTES: u64 = 4096;
const TABLE_ALLOWANCE_BYTES: u64 = 4096;
const MAX_GLOBAL_LIMIT: usize = 1024;
const MAX_TERMINAL_TTL: Duration = Duration::from_secs(3600);
const LOCK_POLL: Duration = Duration::from_millis(1);
const CLOSING_BIT: usize = 1;
const RELEASE_WAITER: usize = 2;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct NativeReadSessionConfig {
    pub global_limit: usize,
    pub per_uid_limit: usize,
    pub terminal_ttl: Duration,
    pub sweep_limit: usize,
}

impl Default for NativeReadSessionConfig {
    fn default() -> Self {
        Self {
            global_limit: 64,
            per_uid_limit: 16,
            terminal_ttl: Duration::from_secs(300),
            sweep_limit: 8,
        }
    }
}

impl NativeReadSessionConfig {
    pub fn validate(self) -> Result<Self, IndexError> {
        if self.global_limit == 0
            || self.global_limit > MAX_GLOBAL_LIMIT
            || self.per_uid_limit == 0
            || self.per_uid_limit > self.global_limit
            || self.sweep_limit == 0
            || self.sweep_limit > self.global_limit
            || self.terminal_ttl.is_zero()
            || self.terminal_ttl > MAX_TERMINAL_TTL
        {
            return Err(IndexError::InvalidInput(
                "invalid bounded native read-session config".into(),
            ));
        }
        Ok(self)
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub(super) struct NativeReadSessionSweep {
    pub(super) examined: usize,
    pub(super) busy: usize,
    pub(super) released: usize,
    pub(super) removed: usize,
    pub(super) failures: usize,
    pub(super) first_error: Option<IndexError>,
}

struct Entry {
    id: NativeReadSessionId,
    uid: u32,
    open_key: [u8; 32],
    selection: ViewSelection,
    // Bit 0 is permanent Closing intent; the remaining bits count public
    // Release attempts. One atomic snapshot owns both sweep decisions.
    lifecycle: AtomicUsize,
    inner: Mutex<EntryState>,
}

struct PublicReleaseAttempt<'a>(&'a AtomicUsize);

impl<'a> PublicReleaseAttempt<'a> {
    fn begin(entry: &'a Entry) -> Result<Self, IndexError> {
        let _prior_lifecycle = entry
            .lifecycle
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |state| {
                state
                    .checked_add(RELEASE_WAITER)
                    .map(|next| next | CLOSING_BIT)
            })
            .map_err(|_overflow| IndexError::ResourceLimited)?;
        Ok(Self(&entry.lifecycle))
    }
}

impl Drop for PublicReleaseAttempt<'_> {
    fn drop(&mut self) {
        let _prior = self.0.fetch_sub(RELEASE_WAITER, Ordering::AcqRel);
    }
}

impl Entry {
    fn is_closing(&self) -> bool {
        self.lifecycle.load(Ordering::Acquire) & CLOSING_BIT != 0
    }

    fn status(&self, state: &EntryState) -> Result<NativeReadSessionStatus, IndexError> {
        let mut status = state.status()?;
        if status.state == NativeReadSessionState::Active && self.is_closing() {
            status.state = NativeReadSessionState::Closing;
        }
        Ok(status)
    }

    fn require_active_query(&self, state: &EntryState) -> Result<(), IndexError> {
        if self.is_closing() || state.status()?.state != NativeReadSessionState::Active {
            return Err(IndexError::SnapshotUnavailable);
        }
        Ok(())
    }
}

#[derive(Default)]
struct EntryState {
    ticket: Option<ReadTicket>,
    status: Option<NativeReadSessionStatus>,
    failed_open: bool,
    last_renew: Option<(u64, NativeReadSessionStatus)>,
    has_remaining_target_pins: Option<bool>,
    public_release_ack: bool,
    terminal_since: Option<Instant>,
}

impl EntryState {
    fn status(&self) -> Result<NativeReadSessionStatus, IndexError> {
        if self.failed_open {
            return Err(IndexError::SnapshotUnavailable);
        }
        self.status.ok_or_else(|| {
            IndexError::BackendViolation("native read session has no settled status".into())
        })
    }

    fn set_state(&mut self, state: NativeReadSessionState) -> Result<(), IndexError> {
        let status = self.status.as_mut().ok_or_else(|| {
            IndexError::BackendViolation("native read session has no ticket status".into())
        })?;
        status.state = state;
        Ok(())
    }
}

#[derive(Default)]
struct Table {
    by_id: BTreeMap<NativeReadSessionId, Arc<Entry>>,
    by_open: BTreeMap<(u32, [u8; 32]), NativeReadSessionId>,
    sweep_cursor: Option<NativeReadSessionId>,
}

/// Retained entries, including uncertain releases and terminal tombstones,
/// consume both the slot cap and the caller-supplied memory grant.
pub(super) struct NativeReadSessions {
    config: NativeReadSessionConfig,
    table: Mutex<Table>,
    _memory: Option<MemoryLease>,
}

impl NativeReadSessions {
    /// Pessimistic logical backing for a full table and its retained entries.
    /// This is an admission charge, not an RSS measurement.
    pub(super) fn required_memory(config: &NativeReadSessionConfig) -> Result<u64, IndexError> {
        let _validated = config.validate()?;
        u64::try_from(config.global_limit)
            .map_err(|_error| IndexError::ResourceLimited)?
            .checked_mul(SLOT_ALLOWANCE_BYTES)
            .and_then(|slots| slots.checked_add(TABLE_ALLOWANCE_BYTES))
            .ok_or(IndexError::ResourceLimited)
    }

    pub(super) fn new(
        config: NativeReadSessionConfig,
        memory: MemoryLease,
    ) -> Result<Self, IndexError> {
        let required = Self::required_memory(&config)?;
        if memory.bytes() < required {
            return Err(IndexError::ResourceLimited);
        }
        Ok(Self {
            config,
            table: Mutex::new(Table::default()),
            _memory: Some(memory),
        })
    }

    #[cfg(test)]
    fn new_unfunded_for_test(config: NativeReadSessionConfig) -> Result<Self, IndexError> {
        let _validated = config.validate()?;
        Ok(Self {
            config,
            table: Mutex::new(Table::default()),
            _memory: None,
        })
    }

    /// Replay of the same `(uid, open_key, selection)` returns the original
    /// handle. A changed selection under the same key is never a new pin.
    pub(super) fn open<P>(
        &self,
        uid: u32,
        open_key: [u8; 32],
        selection: ViewSelection,
        budget: &WorkBudget,
        pin: P,
    ) -> Result<NativeReadSessionStatus, IndexError>
    where
        P: FnOnce(ViewSelection) -> Result<ReadTicket, IndexError>,
    {
        if let Some(existing) = self.find_open(uid, open_key, selection, budget)? {
            let guard = lock_entry(&existing.inner, budget)?;
            return existing.status(&guard);
        }

        for _ in 0..8 {
            budget.checkpoint()?;
            let id = random_id()?;
            let candidate = Arc::new(Entry {
                id,
                uid,
                open_key,
                selection,
                lifecycle: AtomicUsize::new(0),
                inner: Mutex::new(EntryState::default()),
            });
            // Lock before publication: a concurrent replay cannot see an
            // unowned Opening slot or create a second Catalog ticket.
            let mut owned = lock_entry(&candidate.inner, budget)?;
            let mut table = lock_table(&self.table, budget)?;
            if let Some(existing_id) = table.by_open.get(&(uid, open_key)) {
                let existing = table.by_id.get(existing_id).cloned().ok_or_else(|| {
                    IndexError::BackendViolation("native read-session open index drifted".into())
                })?;
                drop(table);
                drop(owned);
                if existing.selection != selection {
                    return Err(IndexError::Conflict);
                }
                let guard = lock_entry(&existing.inner, budget)?;
                return existing.status(&guard);
            }
            if table.by_id.contains_key(&id) {
                continue;
            }
            if table.by_id.len() >= self.config.global_limit
                || table
                    .by_id
                    .values()
                    .filter(|entry| entry.uid == uid)
                    .count()
                    >= self.config.per_uid_limit
            {
                return Err(IndexError::ResourceLimited);
            }
            let _prior_open = table.by_open.insert((uid, open_key), id);
            let _prior_entry = table.by_id.insert(id, Arc::clone(&candidate));
            drop(table);

            match pin(selection) {
                Ok(ticket) => {
                    let status = NativeReadSessionStatus {
                        session: id,
                        view: ticket.view().identity,
                        expires_at_ms: ticket.record().lease.expires_at_ms,
                        revision: 1,
                        state: NativeReadSessionState::Active,
                    };
                    owned.ticket = Some(ticket);
                    owned.status = Some(status);
                    drop(owned);
                    // A lost or cancelled reply retains the idempotent Open
                    // result, including the sole Catalog pin.
                    budget.checkpoint()?;
                    return Ok(status);
                }
                Err(error) => {
                    owned.failed_open = true;
                    drop(owned);
                    let _removed = self.remove_if_same(&candidate);
                    return Err(error);
                }
            }
        }
        Err(IndexError::ResourceLimited)
    }

    pub(super) fn renew(
        &self,
        uid: u32,
        id: NativeReadSessionId,
        revision: u64,
        budget: &WorkBudget,
    ) -> Result<NativeReadSessionStatus, IndexError> {
        let entry = self.lookup(uid, id, budget)?;
        let mut guard = lock_entry(&entry.inner, budget)?;
        let current = entry.status(&guard)?;
        if current.state != NativeReadSessionState::Active {
            return Err(IndexError::SnapshotUnavailable);
        }
        if let Some((prior, reply)) = guard.last_renew
            && prior == revision
        {
            return Ok(reply);
        }
        if revision != current.revision {
            return Err(IndexError::Conflict);
        }
        let next = current
            .revision
            .checked_add(1)
            .ok_or(IndexError::ResourceLimited)?;
        let ticket = guard.ticket.as_mut().ok_or_else(|| {
            IndexError::BackendViolation("active read session lost its ticket".into())
        })?;
        if let Err(error) = ticket.renew() {
            guard.set_state(NativeReadSessionState::Unknown)?;
            return Err(error);
        }
        let reply = NativeReadSessionStatus {
            expires_at_ms: ticket.record().lease.expires_at_ms,
            revision: next,
            ..current
        };
        guard.status = Some(reply);
        guard.last_renew = Some((revision, reply));
        drop(guard);
        budget.checkpoint()?;
        Ok(reply)
    }

    /// A status snapshot reports registry custody, not an independent Catalog
    /// proof that the lease is still valid at this instant.
    pub(super) fn status(
        &self,
        uid: u32,
        id: NativeReadSessionId,
        budget: &WorkBudget,
    ) -> Result<NativeReadSessionStatus, IndexError> {
        let entry = self.lookup(uid, id, budget)?;
        let guard = lock_entry(&entry.inner, budget)?;
        entry.status(&guard)
    }

    /// The entry lock excludes release and renewal for the entire query.
    /// Both Catalog validations are required before returning a result.
    pub(super) fn with_ticket<R, Q>(
        &self,
        uid: u32,
        id: NativeReadSessionId,
        budget: &WorkBudget,
        query: Q,
    ) -> Result<R, IndexError>
    where
        Q: FnOnce(&mut ReadTicket) -> Result<R, IndexError>,
    {
        let entry = self.lookup(uid, id, budget)?;
        if entry.is_closing() {
            return Err(IndexError::SnapshotUnavailable);
        }
        let mut guard = lock_entry(&entry.inner, budget)?;
        // A Release may have announced Closing after the pre-lock check.
        entry.require_active_query(&guard)?;
        budget.checkpoint()?;
        let ticket = guard.ticket.as_mut().ok_or_else(|| {
            IndexError::BackendViolation("active read session lost its ticket".into())
        })?;
        if let Err(error) = ticket.validate() {
            guard.set_state(NativeReadSessionState::Unknown)?;
            return Err(error);
        }
        let result = query(ticket);
        let post = ticket.validate();
        if post.is_err() {
            guard.set_state(NativeReadSessionState::Unknown)?;
        }
        let outcome = match (result, post) {
            (Err(primary), _) => Err(primary),
            (Ok(_), Err(error)) => Err(error),
            (Ok(value), Ok(())) => Ok(value),
        };
        drop(guard);
        if outcome.is_ok() {
            budget.checkpoint()?;
        }
        outcome
    }

    /// The closure must perform the original Kernel ticket release and return
    /// its Catalog-backed remaining-pin observation. Only a confirmed public
    /// release may replay its ACK; maintenance cleanup has separate custody.
    pub(super) fn release<F>(
        &self,
        uid: u32,
        id: NativeReadSessionId,
        budget: &WorkBudget,
        release: F,
    ) -> Result<(NativeReadSessionStatus, bool), IndexError>
    where
        F: FnOnce(&mut ReadTicket) -> Result<bool, IndexError>,
    {
        let entry = self.lookup(uid, id, budget)?;
        budget.checkpoint()?;
        let _attempt = PublicReleaseAttempt::begin(&entry)?;
        // The attempt atomically announced Closing before waiting for an
        // in-flight query. Cancellation drops its waiter but retains intent.
        let mut guard = lock_entry(&entry.inner, budget)?;
        let status = guard.status()?;
        if status.state == NativeReadSessionState::Released {
            if !guard.public_release_ack {
                return Err(IndexError::SnapshotUnavailable);
            }
            return Ok((
                status,
                guard.has_remaining_target_pins.ok_or_else(|| {
                    IndexError::BackendViolation(
                        "released read session lost pin observation".into(),
                    )
                })?,
            ));
        }
        guard.set_state(NativeReadSessionState::Closing)?;
        let ticket = guard.ticket.as_mut().ok_or_else(|| {
            IndexError::BackendViolation("closing read session lost its ticket".into())
        })?;
        let outcome = match release(ticket) {
            Ok(has_remaining) => {
                guard.set_state(NativeReadSessionState::Released)?;
                guard.has_remaining_target_pins = Some(has_remaining);
                guard.public_release_ack = true;
                guard.terminal_since = Some(Instant::now());
                Ok((guard.status()?, has_remaining))
            }
            Err(error) => {
                guard.set_state(NativeReadSessionState::Unknown)?;
                Err(error)
            }
        };
        drop(guard);
        outcome
    }

    /// Observe settlement after a confirmed public Release against the retained
    /// original ticket and process session, never a fresh or client-supplied pin.
    pub(super) fn with_released_ticket<R, O>(
        &self,
        uid: u32,
        id: NativeReadSessionId,
        budget: &WorkBudget,
        observe: O,
    ) -> Result<R, IndexError>
    where
        O: FnOnce(&ReadTicket) -> Result<R, IndexError>,
    {
        let entry = self.lookup(uid, id, budget)?;
        let guard = lock_entry(&entry.inner, budget)?;
        if guard.status()?.state != NativeReadSessionState::Released || !guard.public_release_ack {
            return Err(IndexError::SnapshotUnavailable);
        }
        let ticket = guard.ticket.as_ref().ok_or_else(|| {
            IndexError::BackendViolation("released read session lost retained ticket".into())
        })?;
        budget.checkpoint()?;
        let result = observe(ticket);
        drop(guard);
        if result.is_ok() {
            budget.checkpoint()?;
        }
        result
    }

    /// One existing-pump step. A busy entry is skipped; no table lock spans
    /// Catalog validation, release, or a caller query.
    pub(super) fn sweep<F>(
        &self,
        budget: &WorkBudget,
        mut release: F,
    ) -> Result<NativeReadSessionSweep, IndexError>
    where
        F: FnMut(&mut ReadTicket) -> Result<bool, IndexError>,
    {
        let entries = {
            let mut table = lock_table(&self.table, budget)?;
            let selected = select_sweep(&table, self.config.sweep_limit);
            if let Some(last) = selected.last() {
                table.sweep_cursor = Some(last.id);
            }
            selected
        };
        let mut report = NativeReadSessionSweep::default();
        for entry in entries {
            keep_first(&report, budget.checkpoint())?;
            let mut guard = match entry.inner.try_lock() {
                Ok(guard) => guard,
                Err(TryLockError::WouldBlock) => {
                    report.busy = report.busy.saturating_add(1);
                    continue;
                }
                Err(TryLockError::Poisoned(_)) => {
                    record_failure(&mut report, poisoned());
                    continue;
                }
            };
            report.examined = report.examined.saturating_add(1);
            if guard.failed_open {
                drop(guard);
                if self.remove_if_same(&entry) {
                    report.removed = report.removed.saturating_add(1);
                }
                continue;
            }
            let status = keep_first(&report, guard.status())?;
            if status.state == NativeReadSessionState::Released {
                let expired = guard
                    .terminal_since
                    .is_some_and(|since| since.elapsed() >= self.config.terminal_ttl);
                drop(guard);
                if expired && self.remove_if_same(&entry) {
                    report.removed = report.removed.saturating_add(1);
                }
                continue;
            }
            // Closing and waiter count are one atomic decision. A later public
            // attempt competes only with an already-started maintenance round.
            let lifecycle = entry.lifecycle.load(Ordering::Acquire);
            if lifecycle >= RELEASE_WAITER {
                continue;
            }
            let closing = lifecycle & CLOSING_BIT != 0;
            if status.state == NativeReadSessionState::Active && !closing {
                let ticket = keep_first(
                    &report,
                    guard.ticket.as_ref().ok_or_else(|| {
                        IndexError::BackendViolation("active read session lost its ticket".into())
                    }),
                )?;
                match ticket.validate() {
                    Ok(()) => continue,
                    Err(IndexError::SnapshotUnavailable) => {}
                    Err(error) => {
                        record_failure(&mut report, error);
                        keep_first(&report, guard.set_state(NativeReadSessionState::Unknown))?;
                        continue;
                    }
                }
            }
            keep_first(&report, guard.set_state(NativeReadSessionState::Closing))?;
            let ticket = keep_first(
                &report,
                guard.ticket.as_mut().ok_or_else(|| {
                    IndexError::BackendViolation("closing read session lost its ticket".into())
                }),
            )?;
            match release(ticket) {
                Ok(remaining) => {
                    keep_first(&report, guard.set_state(NativeReadSessionState::Released))?;
                    guard.has_remaining_target_pins = Some(remaining);
                    guard.public_release_ack = false;
                    guard.terminal_since = Some(Instant::now());
                    report.released = report.released.saturating_add(1);
                }
                Err(error) => {
                    record_failure(&mut report, error);
                    keep_first(&report, guard.set_state(NativeReadSessionState::Unknown))?;
                }
            }
        }
        Ok(report)
    }

    /// Shutdown attempts each retained live ticket once and reports every
    /// failure. Failed entries stay in the registry for explicit retry.
    pub(super) fn drain<F>(
        &self,
        budget: &WorkBudget,
        mut release: F,
    ) -> Result<NativeReadSessionSweep, IndexError>
    where
        F: FnMut(&mut ReadTicket) -> Result<bool, IndexError>,
    {
        let entries = {
            let table = lock_table(&self.table, budget)?;
            table.by_id.values().cloned().collect::<Vec<_>>()
        };
        let mut report = NativeReadSessionSweep::default();
        for entry in entries {
            keep_first(&report, budget.checkpoint())?;
            let mut guard = keep_first(&report, lock_entry(&entry.inner, budget))?;
            report.examined = report.examined.saturating_add(1);
            if guard.failed_open {
                continue;
            }
            if keep_first(&report, guard.status())?.state == NativeReadSessionState::Released {
                continue;
            }
            keep_first(&report, guard.set_state(NativeReadSessionState::Closing))?;
            let ticket = keep_first(
                &report,
                guard.ticket.as_mut().ok_or_else(|| {
                    IndexError::BackendViolation("closing read session lost its ticket".into())
                }),
            )?;
            match release(ticket) {
                Ok(remaining) => {
                    keep_first(&report, guard.set_state(NativeReadSessionState::Released))?;
                    guard.has_remaining_target_pins = Some(remaining);
                    guard.public_release_ack = false;
                    guard.terminal_since = Some(Instant::now());
                    report.released = report.released.saturating_add(1);
                }
                Err(error) => {
                    record_failure(&mut report, error);
                    keep_first(&report, guard.set_state(NativeReadSessionState::Unknown))?;
                }
            }
        }
        Ok(report)
    }

    fn lookup(
        &self,
        uid: u32,
        id: NativeReadSessionId,
        budget: &WorkBudget,
    ) -> Result<Arc<Entry>, IndexError> {
        let table = lock_table(&self.table, budget)?;
        table
            .by_id
            .get(&id)
            .filter(|entry| entry.uid == uid)
            .cloned()
            .ok_or(IndexError::SnapshotUnavailable)
    }

    fn find_open(
        &self,
        uid: u32,
        key: [u8; 32],
        selection: ViewSelection,
        budget: &WorkBudget,
    ) -> Result<Option<Arc<Entry>>, IndexError> {
        let table = lock_table(&self.table, budget)?;
        let Some(id) = table.by_open.get(&(uid, key)) else {
            return Ok(None);
        };
        let entry = table.by_id.get(id).cloned().ok_or_else(|| {
            IndexError::BackendViolation("native read-session open index drifted".into())
        })?;
        drop(table);
        if entry.selection != selection {
            return Err(IndexError::Conflict);
        }
        Ok(Some(entry))
    }

    fn remove_if_same(&self, entry: &Arc<Entry>) -> bool {
        match self.table.try_lock() {
            Ok(mut table) => {
                if table
                    .by_id
                    .get(&entry.id)
                    .is_some_and(|current| Arc::ptr_eq(current, entry))
                {
                    let _removed_entry = table.by_id.remove(&entry.id);
                    if table.by_open.get(&(entry.uid, entry.open_key)) == Some(&entry.id) {
                        let _removed_open = table.by_open.remove(&(entry.uid, entry.open_key));
                    }
                    true
                } else {
                    false
                }
            }
            Err(TryLockError::WouldBlock | TryLockError::Poisoned(_)) => false,
        }
        // A contended or poisoned table retains the charged entry. Contention
        // is revisited by the existing pump without extending this request.
    }
}

fn select_sweep(table: &Table, limit: usize) -> Vec<Arc<Entry>> {
    let mut entries = Vec::new();
    if let Some(cursor) = table.sweep_cursor {
        entries.extend(
            table
                .by_id
                .range((Excluded(cursor), Unbounded))
                .take(limit)
                .map(|(_id, entry)| Arc::clone(entry)),
        );
        if entries.len() < limit {
            entries.extend(
                table
                    .by_id
                    .range(..=cursor)
                    .take(limit.saturating_sub(entries.len()))
                    .map(|(_id, entry)| Arc::clone(entry)),
            );
        }
    } else {
        entries.extend(table.by_id.values().take(limit).cloned());
    }
    entries
}

fn record_failure(report: &mut NativeReadSessionSweep, error: IndexError) {
    report.failures = report.failures.saturating_add(1);
    if report.first_error.is_none() {
        report.first_error = Some(error);
    }
}

fn keep_first<T>(
    report: &NativeReadSessionSweep,
    result: Result<T, IndexError>,
) -> Result<T, IndexError> {
    result.map_err(|error| report.first_error.clone().unwrap_or(error))
}

fn random_id() -> Result<NativeReadSessionId, IndexError> {
    let mut bytes = [0_u8; 32];
    File::open("/dev/urandom")
        .and_then(|mut source| source.read_exact(&mut bytes))
        .map_err(|error| IndexError::Storage(format!("native read-session entropy: {error}")))?;
    if bytes == [0; 32] {
        return Err(IndexError::ResourceLimited);
    }
    Ok(NativeReadSessionId(bytes))
}

fn poisoned() -> IndexError {
    IndexError::Storage("native read-session registry lock poisoned".into())
}

fn lock_table<'a>(
    mutex: &'a Mutex<Table>,
    budget: &WorkBudget,
) -> Result<MutexGuard<'a, Table>, IndexError> {
    lock_entry(mutex, budget)
}

fn lock_entry<'a, T>(
    mutex: &'a Mutex<T>,
    budget: &WorkBudget,
) -> Result<MutexGuard<'a, T>, IndexError> {
    loop {
        budget.checkpoint()?;
        match mutex.try_lock() {
            Ok(guard) => return Ok(guard),
            Err(TryLockError::WouldBlock) => {
                thread::sleep(LOCK_POLL.min(budget.request().remaining()));
            }
            Err(TryLockError::Poisoned(_)) => return Err(poisoned()),
        }
    }
}

#[cfg(test)]
#[path = "native_read_sessions/tests.rs"]
mod tests;
