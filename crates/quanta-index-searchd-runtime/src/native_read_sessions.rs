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
        Arc, Mutex, MutexGuard, OnceLock, RwLock, RwLockReadGuard, RwLockWriteGuard, TryLockError,
        atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering},
    },
    thread,
    time::{Duration, Instant},
};

use quanta_index_contract::{
    index::{Digest, IndexInstanceId, ViewRef, ViewSelection},
    native_protocol::{NativeReadSessionId, NativeReadSessionState, NativeReadSessionStatus},
};
use quanta_index_core::index::{
    IndexError, MemoryLease, MemoryLeaseOwner, WorkBudget, WorkBudgetDenial,
};
use quanta_index_kernel::ReadTicket;
use quanta_index_lexical::native::{MAX_TEXT_RANK_ORDER_BYTES, TextRankMemoryPort, TextRankOrder};

const SLOT_ALLOWANCE_BYTES: u64 = 4096;
const TABLE_ALLOWANCE_BYTES: u64 = 4096;
const MAX_GLOBAL_LIMIT: usize = 1024;
const DEFAULT_GLOBAL_LIMIT: usize = 64;
// Bound the replay window separately from concurrent live custody. At the
// defaults, one UID can retain 128 sequential opens and the daemon 512.
// Saturation applies backpressure until TTL rather than breaking replay.
const RETAINED_SLOTS_PER_LIVE_SLOT: usize = 8;
#[expect(
    clippy::as_conversions,
    reason = "a bounded session count widens to u64; u64::try_from is not const"
)]
const DEFAULT_TEXT_ORDER_MEMORY: u64 = DEFAULT_GLOBAL_LIMIT as u64 * MAX_TEXT_RANK_ORDER_BYTES;
const MAX_TERMINAL_TTL: Duration = Duration::from_secs(3600);
const LOCK_POLL: Duration = Duration::from_millis(1);
const CLOSING_BIT: usize = 1;
const RELEASE_WAITER: usize = 2;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct NativeReadSessionConfig {
    /// Concurrent live or opening sessions, including uncertain releases.
    pub global_limit: usize,
    /// Concurrent live or opening sessions for one authenticated UID.
    pub per_uid_limit: usize,
    /// Minimum replay lifetime for a released record. After this TTL, a sweep
    /// or capacity admission may remove the record.
    pub terminal_ttl: Duration,
    pub sweep_limit: usize,
    /// The declared share of the registry's Interactive grant that cached
    /// text rank orders may hold across all sessions; `0` disables the cache.
    ///
    /// A session keeps at most one text page's full rank order so later
    /// pages of the same view, instance and query skip the full re-scan. The
    /// share is reserved with the registry at configuration, so it is checked
    /// against the Interactive plan once and never competes with responses
    /// afterwards. An order the share cannot fund is not kept; its later
    /// pages re-rank, with identical results. A share above
    /// [`Self::full_text_order_memory`] could never be used and is refused.
    pub text_order_memory: u64,
}

impl Default for NativeReadSessionConfig {
    fn default() -> Self {
        Self {
            global_limit: DEFAULT_GLOBAL_LIMIT,
            per_uid_limit: 16,
            terminal_ttl: Duration::from_secs(300),
            sweep_limit: 8,
            text_order_memory: DEFAULT_TEXT_ORDER_MEMORY,
        }
    }
}

/// The identity a cached text order answers: one view, member instance,
/// canonical query digest and projection (owner or `select:path` pages).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) struct TextOrderKey {
    pub(super) view: ViewRef,
    pub(super) instance: IndexInstanceId,
    pub(super) query: Digest,
    pub(super) path: bool,
}

/// At most one cached text order per session. Its rows hold a charge on the
/// registry's text order share, returned when the last holder of the order
/// drops it after the session is released or removed.
type CachedTextOrder = Option<(TextOrderKey, Arc<TextRankOrder>)>;

/// The sub-ledger cached text orders are charged to.
///
/// It is the declared `text_order_memory` share of the registry's
/// Interactive grant. It only counts bytes the registry already reserved, so
/// funding an order never reserves service memory and never competes with
/// responses.
#[derive(Debug)]
struct TextOrderShare {
    capacity: u64,
    used: AtomicU64,
    // A release below zero is corrupted accounting: no later charge trusts it.
    invalid: AtomicBool,
}

impl TextOrderShare {
    fn new(capacity: u64) -> Arc<Self> {
        Arc::new(Self {
            capacity,
            used: AtomicU64::new(0),
            invalid: AtomicBool::new(false),
        })
    }

    /// A charge of `bytes`, or `None` when the share cannot fund them.
    fn charge(self: &Arc<Self>, bytes: u64) -> Option<MemoryLease> {
        if self.invalid.load(Ordering::Acquire) {
            return None;
        }
        if self
            .used
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |used| {
                used.checked_add(bytes)
                    .filter(|charged| *charged <= self.capacity)
            })
            .is_err()
        {
            return None;
        }
        let owner: Arc<dyn MemoryLeaseOwner> = Arc::<Self>::clone(self);
        Some(MemoryLease::new(bytes, owner))
    }

    #[cfg(test)]
    fn used(&self) -> u64 {
        self.used.load(Ordering::Acquire)
    }
}

impl MemoryLeaseOwner for TextOrderShare {
    fn release(&self, bytes: u64) {
        if self
            .used
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |used| {
                used.checked_sub(bytes)
            })
            .is_err()
        {
            self.invalid.store(true, Ordering::Release);
        }
    }
}

impl TextRankMemoryPort for TextOrderShare {
    fn try_reserve(self: Arc<Self>, bytes: u64) -> Result<Option<MemoryLease>, IndexError> {
        if self.invalid.load(Ordering::Acquire) {
            return Err(IndexError::BackendViolation(
                "text order share ledger is invalid".into(),
            ));
        }
        let charge = self.charge(bytes);
        if charge.is_none() && self.invalid.load(Ordering::Acquire) {
            return Err(IndexError::BackendViolation(
                "text order share ledger is invalid".into(),
            ));
        }
        Ok(charge)
    }
}

impl NativeReadSessionConfig {
    /// The share that lets every one of `global_limit` sessions keep the
    /// largest order a capture produces.
    pub fn full_text_order_memory(global_limit: usize) -> Result<u64, IndexError> {
        u64::try_from(global_limit)
            .map_err(|_width| IndexError::ResourceLimited)?
            .checked_mul(MAX_TEXT_RANK_ORDER_BYTES)
            .ok_or(IndexError::ResourceLimited)
    }

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
        if self.text_order_memory > Self::full_text_order_memory(self.global_limit)? {
            return Err(IndexError::InvalidInput(
                "native read-session text order share exceeds every session's largest order".into(),
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
    // A queued Renew fences new query readers before waiting for a write lock.
    renew_waiters: AtomicUsize,
    // Validation uncertainty must become visible before a shared query guard
    // is released; an expired caller cannot wait for a write lock to mark it.
    uncertain: AtomicBool,
    // Published only after a confirmed release and its replay data are set.
    // Open can classify entries without taking their locks under the table lock.
    terminal_since: OnceLock<Instant>,
    inner: RwLock<EntryState>,
    text_order: Mutex<CachedTextOrder>,
}

struct RenewWaiter<'a>(&'a AtomicUsize);

impl<'a> RenewWaiter<'a> {
    fn begin(entry: &'a Entry) -> Result<Self, IndexError> {
        let _prior = entry
            .renew_waiters
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |count| {
                count.checked_add(1)
            })
            .map_err(|_overflow| IndexError::ResourceLimited)?;
        Ok(Self(&entry.renew_waiters))
    }
}

impl Drop for RenewWaiter<'_> {
    fn drop(&mut self) {
        let _prior = self.0.fetch_sub(1, Ordering::AcqRel);
    }
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
    fn mark_released(&self) {
        let _since = self.terminal_since.get_or_init(Instant::now);
    }

    /// Drop the cached order with its memory grant.
    fn clear_text_order(&self) {
        let cached = match self.text_order.lock() {
            Ok(mut cached) => cached.take(),
            Err(poisoned) => poisoned.into_inner().take(),
        };
        drop(cached);
    }

    fn is_closing(&self) -> bool {
        self.lifecycle.load(Ordering::Acquire) & CLOSING_BIT != 0
    }

    fn has_waiting_writer(&self) -> bool {
        self.renew_waiters.load(Ordering::Acquire) != 0
            || self.lifecycle.load(Ordering::Acquire) & !CLOSING_BIT != 0
    }

    fn status(&self, state: &EntryState) -> Result<NativeReadSessionStatus, IndexError> {
        let mut status = state.status()?;
        if status.state == NativeReadSessionState::Active && self.is_closing() {
            status.state = NativeReadSessionState::Closing;
        } else if status.state == NativeReadSessionState::Active
            && self.uncertain.load(Ordering::Acquire)
        {
            status.state = NativeReadSessionState::Unknown;
        }
        Ok(status)
    }

    fn require_active_query(&self, state: &EntryState) -> Result<(), IndexError> {
        if self.status(state)?.state != NativeReadSessionState::Active {
            return Err(IndexError::SnapshotUnavailable);
        }
        Ok(())
    }

    fn record_query_validation_failure(&self, budget: &WorkBudget, error: &IndexError) {
        if !matches!(
            (budget.first_denial(), error),
            (Some(WorkBudgetDenial::Cancelled), IndexError::Cancelled)
                | (
                    Some(WorkBudgetDenial::Deadline),
                    IndexError::ResourceLimited
                )
        ) {
            self.uncertain.store(true, Ordering::Release);
        }
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
}

impl EntryState {
    fn record_ticket_failure(
        &mut self,
        budget: &WorkBudget,
        error: &IndexError,
    ) -> Result<(), IndexError> {
        match (budget.first_denial(), error) {
            (Some(WorkBudgetDenial::Cancelled), IndexError::Cancelled)
            | (Some(WorkBudgetDenial::Deadline), IndexError::ResourceLimited) => Ok(()),
            _ => self.set_state(NativeReadSessionState::Unknown),
        }
    }

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

    fn record_maintenance_release(&mut self, remaining: bool) -> Result<(), IndexError> {
        self.set_state(NativeReadSessionState::Released)?;
        self.has_remaining_target_pins = Some(remaining);
        self.public_release_ack = false;
        Ok(())
    }
}

#[derive(Default)]
struct Table {
    by_id: BTreeMap<NativeReadSessionId, Arc<Entry>>,
    by_open: BTreeMap<(u32, [u8; 32]), NativeReadSessionId>,
    sweep_cursor: Option<NativeReadSessionId>,
}

impl Table {
    fn make_room_for_open(
        &mut self,
        uid: u32,
        config: &NativeReadSessionConfig,
    ) -> Result<(), IndexError> {
        let mut live = 0;
        let mut live_for_uid = 0;
        let mut retained_for_uid = 0;
        for entry in self.by_id.values() {
            let is_live = entry.terminal_since.get().is_none();
            live += usize::from(is_live);
            if entry.uid == uid {
                live_for_uid += usize::from(is_live);
                retained_for_uid += 1;
            }
        }
        if live >= config.global_limit || live_for_uid >= config.per_uid_limit {
            return Err(IndexError::ResourceLimited);
        }

        let uid_full = retained_for_uid >= config.per_uid_limit * RETAINED_SLOTS_PER_LIVE_SLOT;
        let table_full = self.by_id.len() >= config.global_limit * RETAINED_SLOTS_PER_LIVE_SLOT;
        if uid_full || table_full {
            // Do not evict before the replay TTL or while a concurrent
            // Release, settlement observation or sweep holds the record. A
            // new lookup needs this same table lock.
            let (_, oldest) = self
                .by_id
                .values()
                .filter(|entry| !uid_full || entry.uid == uid)
                .filter(|entry| Arc::strong_count(entry) == 1)
                .filter_map(|entry| entry.terminal_since.get().map(|since| (*since, entry.id)))
                .filter(|(since, _id)| since.elapsed() >= config.terminal_ttl)
                .min()
                .ok_or(IndexError::ResourceLimited)?;
            let _removed = self.remove(oldest);
        }
        Ok(())
    }

    fn remove(&mut self, id: NativeReadSessionId) -> bool {
        let Some(entry) = self.by_id.remove(&id) else {
            return false;
        };
        let key = (entry.uid, entry.open_key);
        if self.by_open.get(&key) == Some(&id) {
            let _removed_open = self.by_open.remove(&key);
        }
        true
    }
}

/// Live custody and terminal replay share a funded registry. Confirmed
/// releases return live capacity without dropping recent idempotency records.
pub(super) struct NativeReadSessions {
    config: NativeReadSessionConfig,
    table: Mutex<Table>,
    orders: Arc<TextOrderShare>,
    _memory: Option<MemoryLease>,
}

impl NativeReadSessions {
    /// Pessimistic logical backing for a full table and its retained entries,
    /// plus the declared text order share. This is an admission charge, not
    /// an RSS measurement.
    pub(super) fn required_memory(config: &NativeReadSessionConfig) -> Result<u64, IndexError> {
        let _validated = config.validate()?;
        u64::try_from(config.global_limit * RETAINED_SLOTS_PER_LIVE_SLOT)
            .map_err(|_error| IndexError::ResourceLimited)?
            .checked_mul(SLOT_ALLOWANCE_BYTES)
            .and_then(|slots| slots.checked_add(TABLE_ALLOWANCE_BYTES))
            .and_then(|table| table.checked_add(config.text_order_memory))
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
            orders: TextOrderShare::new(config.text_order_memory),
            _memory: Some(memory),
        })
    }

    #[cfg(test)]
    fn new_unfunded_for_test(config: NativeReadSessionConfig) -> Result<Self, IndexError> {
        let _validated = config.validate()?;
        Ok(Self {
            config,
            table: Mutex::new(Table::default()),
            orders: TextOrderShare::new(config.text_order_memory),
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
            let guard = lock_entry_read(&existing, budget)?;
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
                renew_waiters: AtomicUsize::new(0),
                uncertain: AtomicBool::new(false),
                terminal_since: OnceLock::new(),
                inner: RwLock::new(EntryState::default()),
                text_order: Mutex::new(None),
            });
            // Lock before publication: a concurrent replay cannot see an
            // unowned Opening slot or create a second Catalog ticket.
            let mut owned = lock_entry_write(&candidate.inner, budget)?;
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
                let guard = lock_entry_read(&existing, budget)?;
                return existing.status(&guard);
            }
            if table.by_id.contains_key(&id) {
                continue;
            }
            table.make_room_for_open(uid, &self.config)?;
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
                    checkpoint_open_reply(budget)?;
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
        let _waiter = RenewWaiter::begin(&entry)?;
        let mut guard = lock_entry_write(&entry.inner, budget)?;
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
        if let Err(error) = ticket.renew_for_request(budget) {
            // The request's own checkpoint rejects before Catalog commit. The
            // old lease stays authoritative and the same revision may retry.
            // Other failures can leave commit outcome uncertain.
            guard.record_ticket_failure(budget, &error)?;
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
        let guard = lock_entry_read(&entry, budget)?;
        entry.status(&guard)
    }

    /// The session's cached text order for `key`, if it holds one. A first
    /// page for another key releases the old order before capture can reserve
    /// its share; a continuation miss leaves the old order intact.
    pub(super) fn cached_text_order(
        &self,
        uid: u32,
        id: NativeReadSessionId,
        key: TextOrderKey,
        evict_on_miss: bool,
        budget: &WorkBudget,
    ) -> Result<Option<Arc<TextRankOrder>>, IndexError> {
        if self.config.text_order_memory == 0 {
            return Ok(None);
        }
        let entry = self.lookup(uid, id, budget)?;
        let (hit, evicted) = {
            let mut cached = entry.text_order.lock().map_err(|_poison| {
                IndexError::BackendViolation("text order cache poisoned".into())
            })?;
            if let Some((cached_key, order)) = cached.as_ref()
                && *cached_key == key
            {
                (Some(Arc::clone(order)), None)
            } else {
                (None, if evict_on_miss { cached.take() } else { None })
            }
        };
        drop(evicted);
        Ok(hit)
    }

    /// Whether pages may capture an order for this session table.
    pub(super) const fn text_order_cache_enabled(&self) -> bool {
        self.config.text_order_memory != 0
    }

    pub(super) fn text_order_capture_port(&self) -> Arc<dyn TextRankMemoryPort> {
        Arc::clone(&self.orders) as Arc<dyn TextRankMemoryPort>
    }

    /// Keep `order` as the session's only cached text order, replacing any
    /// other, charged to the text order share. A session that is no longer
    /// active keeps none, and an order the share cannot fund is dropped: its
    /// later pages re-rank, with identical results.
    pub(super) fn retain_text_order(
        &self,
        uid: u32,
        id: NativeReadSessionId,
        key: TextOrderKey,
        order: TextRankOrder,
        budget: &WorkBudget,
    ) -> Result<(), IndexError> {
        if self.config.text_order_memory == 0 {
            return Ok(());
        }
        let entry = self.lookup(uid, id, budget)?;
        if entry.is_closing() || entry.uncertain.load(Ordering::Acquire) {
            return Ok(());
        }
        let mut cached = entry
            .text_order
            .lock()
            .map_err(|_poison| IndexError::BackendViolation("text order cache poisoned".into()))?;
        // The session's previous order leaves the share before the next one
        // is funded.
        drop(cached.take());
        *cached = Some((key, Arc::new(order)));
        drop(cached);
        Ok(())
    }

    /// Preparation takes no entry lock. Shared query custody excludes renewal
    /// and release while allowing concurrent reads of one original ticket.
    /// Hard validation failure is projected as Unknown before releasing it.
    pub(super) fn with_ticket<T, R, P, Q>(
        &self,
        uid: u32,
        id: NativeReadSessionId,
        budget: &WorkBudget,
        prepare: P,
        query: Q,
    ) -> Result<R, IndexError>
    where
        P: FnOnce() -> Result<T, IndexError>,
        Q: FnOnce(
            &ReadTicket,
            &mut quanta_index_kernel::TicketReadAttempt<'_>,
            T,
        ) -> Result<R, IndexError>,
    {
        let entry = self.lookup(uid, id, budget)?;
        if entry.is_closing() || entry.uncertain.load(Ordering::Acquire) {
            return Err(IndexError::SnapshotUnavailable);
        }
        let prepared = prepare()?;
        let guard = lock_query_entry(&entry, budget)?;
        // Release, Renew, or a hard validation failure may have arrived while
        // preparation waited for Control admission.
        entry.require_active_query(&guard)?;
        budget.checkpoint()?;
        let ticket = guard.ticket.as_ref().ok_or_else(|| {
            IndexError::BackendViolation("active read session lost its ticket".into())
        })?;
        let mut attempt = ticket.begin_read_attempt(budget).inspect_err(|error| {
            entry.record_query_validation_failure(budget, error);
        })?;
        if entry.uncertain.load(Ordering::Acquire) {
            attempt.abort();
            return Err(IndexError::SnapshotUnavailable);
        }
        let result = query(ticket, &mut attempt, prepared);
        let post = attempt.finalize();
        if let Err(error) = &post {
            entry.record_query_validation_failure(budget, error);
        }
        let outcome = match (result, post) {
            (Err(primary), _) => Err(primary),
            (Ok(_), Err(error)) => Err(error),
            (Ok(value), Ok(())) if entry.uncertain.load(Ordering::Acquire) => {
                Err(IndexError::SnapshotUnavailable)
            }
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
        let mut guard = lock_entry_write(&entry.inner, budget)?;
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
                entry.clear_text_order();
                guard.set_state(NativeReadSessionState::Released)?;
                guard.has_remaining_target_pins = Some(has_remaining);
                guard.public_release_ack = true;
                entry.mark_released();
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
        let guard = lock_entry_read(&entry, budget)?;
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
            let mut guard = match entry.inner.try_write() {
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
            let status = keep_first(&report, entry.status(&guard))?;
            if status.state == NativeReadSessionState::Released {
                let expired = entry
                    .terminal_since
                    .get()
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
                    // A live quarantined view still has a valid custody lease.
                    // The serving path reports corruption and normal release
                    // or expiry settles the original pin.
                    Ok(()) | Err(IndexError::CorruptObject(_)) => continue,
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
                    entry.clear_text_order();
                    keep_first(&report, guard.record_maintenance_release(remaining))?;
                    entry.mark_released();
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
            let mut guard = keep_first(&report, lock_entry_write(&entry.inner, budget))?;
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
                    entry.clear_text_order();
                    keep_first(&report, guard.record_maintenance_release(remaining))?;
                    entry.mark_released();
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
                    table.remove(entry.id)
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

/// The pin may already be committed when the response deadline expires.
/// Use the cancellation outcome so clients preserve the Open key for replay.
pub(super) fn checkpoint_open_reply(budget: &WorkBudget) -> Result<(), IndexError> {
    match budget.checkpoint() {
        Err(IndexError::ResourceLimited) => Err(IndexError::Cancelled),
        other => other,
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

fn lock_entry_read<'a>(
    entry: &'a Entry,
    budget: &WorkBudget,
) -> Result<RwLockReadGuard<'a, EntryState>, IndexError> {
    loop {
        budget.checkpoint()?;
        if !entry.has_waiting_writer() {
            match entry.inner.try_read() {
                Ok(guard) if !entry.has_waiting_writer() => return Ok(guard),
                Ok(guard) => drop(guard),
                Err(TryLockError::WouldBlock) => {}
                Err(TryLockError::Poisoned(_)) => return Err(poisoned()),
            }
        }
        thread::sleep(LOCK_POLL.min(budget.request().remaining()));
    }
}

fn lock_entry_write<'a>(
    entry: &'a RwLock<EntryState>,
    budget: &WorkBudget,
) -> Result<RwLockWriteGuard<'a, EntryState>, IndexError> {
    loop {
        budget.checkpoint()?;
        match entry.try_write() {
            Ok(guard) => return Ok(guard),
            Err(TryLockError::WouldBlock) => {
                thread::sleep(LOCK_POLL.min(budget.request().remaining()));
            }
            Err(TryLockError::Poisoned(_)) => return Err(poisoned()),
        }
    }
}

fn lock_query_entry<'a>(
    entry: &'a Entry,
    budget: &WorkBudget,
) -> Result<RwLockReadGuard<'a, EntryState>, IndexError> {
    loop {
        budget.checkpoint()?;
        if entry.is_closing() || entry.uncertain.load(Ordering::Acquire) {
            return Err(IndexError::SnapshotUnavailable);
        }
        if entry.renew_waiters.load(Ordering::Acquire) != 0 {
            thread::sleep(LOCK_POLL.min(budget.request().remaining()));
            continue;
        }
        match entry.inner.try_read() {
            Ok(guard) => {
                if entry.renew_waiters.load(Ordering::Acquire) == 0 {
                    return Ok(guard);
                }
                drop(guard);
            }
            Err(TryLockError::WouldBlock) => {}
            Err(TryLockError::Poisoned(_)) => return Err(poisoned()),
        }
        thread::sleep(LOCK_POLL.min(budget.request().remaining()));
    }
}

#[cfg(test)]
#[path = "native_read_sessions/tests.rs"]
mod tests;
