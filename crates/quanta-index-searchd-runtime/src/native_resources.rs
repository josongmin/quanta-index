//! One logical reservation authority for the opt-in native service.
//!
//! These estimates bound concurrent admitted work; they are not RSS, WAL, or
//! physical disk measurements. Request deadlines remain in `WorkBudget`.

use std::{
    collections::BTreeMap,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
};

use quanta_index_core::index::{
    IndexError, MemoryLease, MemoryLeaseOwner, MemoryLifetime, MemoryScope, MemoryScopePort,
    effort::{ResourceNeed, ResourceScheduler},
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct NativeResourcePlan {
    /// Shared execution and retained-data capacity. Memory must also leave
    /// headroom beyond enabled execution floors for any escaping allocation.
    pub capacity: ResourceNeed,
    /// One protected control operation's logical reservation.
    pub control: ResourceNeed,
    pub interactive: ResourceNeed,
    pub core: ResourceNeed,
    pub enrichment: ResourceNeed,
}

impl NativeResourcePlan {
    /// Minimum free headroom after every enabled execution floor is protected.
    /// This is an empty-ledger admission bound, not an RSS or throughput claim.
    pub(crate) fn retained_memory_headroom(
        self,
        owned_pump: bool,
        maintenance: bool,
    ) -> Result<u64, IndexError> {
        let mut floor_bytes = 0_u64;
        for need in [self.control, self.interactive, self.core, self.enrichment] {
            floor_bytes = floor_bytes
                .checked_add(need.memory_bytes)
                .ok_or(IndexError::ResourceLimited)?;
        }
        for enabled in [owned_pump, maintenance] {
            if enabled {
                floor_bytes = floor_bytes
                    .checked_add(self.control.memory_bytes)
                    .ok_or(IndexError::ResourceLimited)?;
            }
        }
        self.capacity
            .memory_bytes
            .checked_sub(floor_bytes)
            .ok_or(IndexError::ResourceLimited)
    }

    pub fn validate(self) -> Result<(), IndexError> {
        let cap = self.capacity;
        if cap.cpu_units == 0 || cap.io_bytes == 0 || cap.memory_bytes == 0 {
            return Err(IndexError::InvalidInput(
                "native resource capacity must include CPU, IO, and memory".into(),
            ));
        }
        let mut floors = ResourceNeed::default();
        for need in [self.control, self.interactive, self.core, self.enrichment] {
            if need.cpu_units == 0 || need.io_bytes == 0 || need.memory_bytes == 0 {
                return Err(IndexError::InvalidInput(
                    "native resource estimate must include CPU, IO, and memory".into(),
                ));
            }
            if !fits(need, cap) {
                return Err(IndexError::InvalidInput(
                    "native resource estimate exceeds service capacity".into(),
                ));
            }
            floors = add(floors, need).ok_or_else(|| {
                IndexError::InvalidInput("native protected floors overflow".into())
            })?;
        }
        if !fits(floors, cap) {
            return Err(IndexError::InvalidInput(
                "native protected floors exceed service capacity".into(),
            ));
        }
        Ok(())
    }

    /// An owned pump needs a separate background-control slot. Its scans and
    /// lease renewals must preserve the foreground Control reservation.
    pub fn validate_owned_pump(self) -> Result<(), IndexError> {
        self.validate_parallel_owned_pump(1, self.enrichment)
    }

    /// Preserve one maintenance Control slot in addition to the foreground
    /// floors and, when enabled, the owned pump's background Control slot.
    pub fn validate_maintenance(self, owned_pump: bool) -> Result<(), IndexError> {
        self.validate()?;
        let mut remaining = self;
        remaining.capacity = subtract(self.capacity, self.control).ok_or_else(|| {
            IndexError::InvalidInput("native maintenance Control floor exceeds capacity".into())
        })?;
        if owned_pump {
            remaining.validate_owned_pump()
        } else {
            remaining.validate()
        }
    }

    /// The maintenance slot and all configured enrichment slots must fit in
    /// the same ledger. This cannot be checked with two separate pools.
    pub fn validate_parallel_maintenance(
        self,
        max_in_flight: u32,
        enrichment_demand: ResourceNeed,
    ) -> Result<(), IndexError> {
        self.validate_maintenance(true)?;
        let mut remaining = self;
        remaining.capacity = subtract(self.capacity, self.control).ok_or_else(|| {
            IndexError::InvalidInput("native maintenance Control floor exceeds capacity".into())
        })?;
        remaining.validate_parallel_owned_pump(max_in_flight, enrichment_demand)
    }

    /// Preserve one foreground slot of every class while admitting the
    /// explicitly configured number of concurrent enrichment executions.
    /// One configured enrichment class floor remains protected even if its
    /// current adapter needs no provider. Additional concurrent slots use the
    /// worker's actual frozen provider demand and fixed CPU/IO/memory envelope.
    pub fn validate_parallel_owned_pump(
        self,
        max_in_flight: u32,
        enrichment_demand: ResourceNeed,
    ) -> Result<(), IndexError> {
        self.validate()?;
        if !(1..=256).contains(&max_in_flight) {
            return Err(IndexError::InvalidInput(
                "native enrichment concurrency must be 1..=256".into(),
            ));
        }
        if !enrichment_demand.fits(self.enrichment) {
            return Err(IndexError::InvalidInput(
                "native enrichment builder demand exceeds its execution class".into(),
            ));
        }
        let enrichment_need = ResourceNeed {
            provider_units: enrichment_demand.provider_units,
            ..self.enrichment
        };
        let mut floors = ResourceNeed::default();
        for need in [
            self.control,
            self.control,
            self.interactive,
            self.core,
            self.enrichment,
        ] {
            floors = add(floors, need).ok_or_else(|| {
                IndexError::InvalidInput("native owned pump floors overflow".into())
            })?;
        }
        for _ in 1..max_in_flight {
            floors = add(floors, enrichment_need).ok_or_else(|| {
                IndexError::InvalidInput("native parallel enrichment floors overflow".into())
            })?;
        }
        if !fits(floors, self.capacity) {
            return Err(IndexError::InvalidInput(
                "native owned pump must preserve foreground Control capacity".into(),
            ));
        }
        Ok(())
    }

    const fn need(self, class: NativeResourceClass) -> ResourceNeed {
        match class {
            NativeResourceClass::Control
            | NativeResourceClass::BackgroundControl
            | NativeResourceClass::MaintenanceControl => self.control,
            NativeResourceClass::Interactive => self.interactive,
            NativeResourceClass::Core => self.core,
            NativeResourceClass::Enrichment => self.enrichment,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct NativeResourceSnapshot {
    pub capacity: ResourceNeed,
    /// Active execution envelopes; detached allocations do not occupy a class.
    pub reserved: ResourceNeed,
    /// Admitted allocation bytes, active and detached, without double charging
    /// them inside an active execution envelope. This is not process RSS.
    pub live_memory_bytes: u64,
    pub retained_memory_bytes: u64,
    /// Future floor/headroom commitment for allocations that may escape.
    pub pending_escape_bytes: u64,
    pub control: NativeClassSnapshot,
    pub background_control: NativeClassSnapshot,
    pub maintenance_control: NativeClassSnapshot,
    pub interactive: NativeClassSnapshot,
    pub core: NativeClassSnapshot,
    pub enrichment: NativeClassSnapshot,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct NativeClassSnapshot {
    pub active: u32,
    pub reserved: ResourceNeed,
}

#[derive(Clone)]
pub struct NativeResourceObservation {
    ledger: Arc<NativeResourceLedger>,
}

impl NativeResourceObservation {
    pub fn observe(&self) -> Result<NativeResourceSnapshot, IndexError> {
        self.ledger.snapshot()
    }
}

pub(super) struct NativeResourceLedger {
    plan: NativeResourcePlan,
    background_control_enabled: bool,
    maintenance_control_enabled: bool,
    state: Mutex<LedgerState>,
    invalid: AtomicBool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum NativeResourceClass {
    Control,
    BackgroundControl,
    MaintenanceControl,
    Interactive,
    Core,
    Enrichment,
}

impl NativeResourceClass {
    const ALL: [Self; 6] = [
        Self::Control,
        Self::BackgroundControl,
        Self::MaintenanceControl,
        Self::Interactive,
        Self::Core,
        Self::Enrichment,
    ];
}

#[derive(Default)]
struct LedgerState {
    total: ResourceNeed,
    next_reservation: u64,
    allocations: BTreeMap<u64, AllocationState>,
    live_memory_bytes: u64,
    retained_memory_bytes: u64,
    pending_escape_bytes: u64,
    control: NativeClassSnapshot,
    background_control: NativeClassSnapshot,
    maintenance_control: NativeClassSnapshot,
    interactive: NativeClassSnapshot,
    core: NativeClassSnapshot,
    enrichment: NativeClassSnapshot,
}

#[derive(Clone, Copy)]
struct AllocationState {
    class: NativeResourceClass,
    need: ResourceNeed,
    active: bool,
    temporary_bytes: u64,
    retained_bytes: u64,
}

impl LedgerState {
    const fn class(&self, class: NativeResourceClass) -> &NativeClassSnapshot {
        match class {
            NativeResourceClass::Control => &self.control,
            NativeResourceClass::BackgroundControl => &self.background_control,
            NativeResourceClass::MaintenanceControl => &self.maintenance_control,
            NativeResourceClass::Interactive => &self.interactive,
            NativeResourceClass::Core => &self.core,
            NativeResourceClass::Enrichment => &self.enrichment,
        }
    }

    const fn class_mut(&mut self, class: NativeResourceClass) -> &mut NativeClassSnapshot {
        match class {
            NativeResourceClass::Control => &mut self.control,
            NativeResourceClass::BackgroundControl => &mut self.background_control,
            NativeResourceClass::MaintenanceControl => &mut self.maintenance_control,
            NativeResourceClass::Interactive => &mut self.interactive,
            NativeResourceClass::Core => &mut self.core,
            NativeResourceClass::Enrichment => &mut self.enrichment,
        }
    }
}

/// A class-bound view of the single service ledger, not another resource pool.
pub(super) struct NativeClassScheduler {
    ledger: Arc<NativeResourceLedger>,
    class: NativeResourceClass,
}

/// A token cannot be cloned. `ReservationGuard` consumes it exactly once.
pub(super) struct NativeResourceToken {
    class: NativeResourceClass,
    need: ResourceNeed,
    reservation: u64,
}

impl NativeClassScheduler {
    pub(super) fn denial(&self) -> IndexError {
        self.ledger.denial()
    }
}

impl NativeResourceLedger {
    #[cfg(test)]
    pub(super) fn new(plan: NativeResourcePlan) -> Result<Arc<Self>, IndexError> {
        Self::with_background_control(plan, false)
    }

    #[cfg(test)]
    pub(super) fn with_background_control(
        plan: NativeResourcePlan,
        enabled: bool,
    ) -> Result<Arc<Self>, IndexError> {
        Self::with_maintenance(plan, enabled, false)
    }

    pub(super) fn with_maintenance(
        plan: NativeResourcePlan,
        owned_pump_enabled: bool,
        maintenance_enabled: bool,
    ) -> Result<Arc<Self>, IndexError> {
        match (owned_pump_enabled, maintenance_enabled) {
            (true, true) => plan.validate_maintenance(true)?,
            (true, false) => plan.validate_owned_pump()?,
            (false, true) => plan.validate_maintenance(false)?,
            (false, false) => plan.validate()?,
        }
        Ok(Arc::new(Self {
            plan,
            background_control_enabled: owned_pump_enabled,
            maintenance_control_enabled: maintenance_enabled,
            state: Mutex::new(LedgerState::default()),
            invalid: AtomicBool::new(false),
        }))
    }

    pub(super) fn scheduler(
        self: &Arc<Self>,
        class: NativeResourceClass,
    ) -> Arc<NativeClassScheduler> {
        Arc::new(NativeClassScheduler {
            ledger: Arc::clone(self),
            class,
        })
    }

    pub(super) fn observation(self: &Arc<Self>) -> NativeResourceObservation {
        NativeResourceObservation {
            ledger: Arc::clone(self),
        }
    }

    pub(super) fn snapshot(&self) -> Result<NativeResourceSnapshot, IndexError> {
        if self.invalid.load(Ordering::Acquire) {
            return Err(invalid_ledger());
        }
        let state = self.state.lock().map_err(|_poison| invalid_ledger())?;
        if self.invalid.load(Ordering::Acquire) {
            return Err(invalid_ledger());
        }
        Ok(NativeResourceSnapshot {
            capacity: self.plan.capacity,
            reserved: state.total,
            live_memory_bytes: state.live_memory_bytes,
            retained_memory_bytes: state.retained_memory_bytes,
            pending_escape_bytes: state.pending_escape_bytes,
            control: state.control,
            background_control: state.background_control,
            maintenance_control: state.maintenance_control,
            interactive: state.interactive,
            core: state.core,
            enrichment: state.enrichment,
        })
    }

    pub(super) fn denial(&self) -> IndexError {
        match self.snapshot() {
            Ok(_) => IndexError::ResourceLimited,
            Err(error) => error,
        }
    }
}

impl ResourceScheduler for NativeClassScheduler {
    type Token = NativeResourceToken;

    fn try_reserve(&self, need: ResourceNeed) -> Option<Self::Token> {
        if self.class == NativeResourceClass::BackgroundControl
            && !self.ledger.background_control_enabled
        {
            return None;
        }
        if self.class == NativeResourceClass::MaintenanceControl
            && !self.ledger.maintenance_control_enabled
        {
            return None;
        }
        let envelope = self.ledger.plan.need(self.class);
        let admitted_need = match self.class {
            NativeResourceClass::Interactive | NativeResourceClass::Enrichment => {
                need.cpu_units > 0
                    && need.io_bytes > 0
                    && need.memory_bytes > 0
                    && fits(need, envelope)
            }
            NativeResourceClass::Core => {
                need.cpu_units == envelope.cpu_units
                    && need.io_bytes == envelope.io_bytes
                    && need.memory_bytes == envelope.memory_bytes
                    && need.provider_units <= envelope.provider_units
            }
            NativeResourceClass::Control
            | NativeResourceClass::BackgroundControl
            | NativeResourceClass::MaintenanceControl => need == envelope,
        };
        if !admitted_need || self.ledger.invalid.load(Ordering::Acquire) {
            return None;
        }
        let Ok(mut state) = self.ledger.state.lock() else {
            self.ledger.invalid.store(true, Ordering::Release);
            return None;
        };
        if self.ledger.invalid.load(Ordering::Acquire) {
            return None;
        }
        let next_total = add(state.total, need)?;
        let current_class = *state.class(self.class);
        let next_class = add(current_class.reserved, need)?;
        let next_count = current_class.active.checked_add(1)?;
        let mut promised = next_total;
        promised.memory_bytes = promised
            .memory_bytes
            .checked_add(state.retained_memory_bytes)?
            .checked_add(state.pending_escape_bytes)?;
        for class in NativeResourceClass::ALL {
            if class == NativeResourceClass::BackgroundControl
                && !self.ledger.background_control_enabled
            {
                continue;
            }
            if class == NativeResourceClass::MaintenanceControl
                && !self.ledger.maintenance_control_enabled
            {
                continue;
            }
            let reserved = if class == self.class {
                next_class
            } else {
                state.class(class).reserved
            };
            promised = add(
                promised,
                floor_shortfall(self.ledger.plan.need(class), reserved),
            )?;
        }
        if !fits(promised, self.ledger.plan.capacity) {
            return None;
        }
        let reservation = state.next_reservation.checked_add(1)?;
        state.next_reservation = reservation;
        state.total = next_total;
        *state.class_mut(self.class) = NativeClassSnapshot {
            active: next_count,
            reserved: next_class,
        };
        let _previous = state.allocations.insert(
            reservation,
            AllocationState {
                class: self.class,
                need,
                active: true,
                temporary_bytes: 0,
                retained_bytes: 0,
            },
        );
        Some(NativeResourceToken {
            class: self.class,
            need,
            reservation,
        })
    }

    fn release(&self, token: Self::Token) {
        if let Err(_error) = self.ledger.finish_execution(self.class, &token) {
            self.ledger.invalid.store(true, Ordering::Release);
        }
    }

    fn memory_scope(&self, token: &Self::Token) -> Option<MemoryScope> {
        if token.class != self.class {
            self.ledger.invalid.store(true, Ordering::Release);
        }
        Some(MemoryScope::new(Arc::new(NativeMemoryScope {
            ledger: Arc::clone(&self.ledger),
            reservation: token.reservation,
        })))
    }
}

struct NativeMemoryScope {
    ledger: Arc<NativeResourceLedger>,
    reservation: u64,
}

impl MemoryScopePort for NativeMemoryScope {
    fn reserve(&self, bytes: u64, lifetime: MemoryLifetime) -> Result<MemoryLease, IndexError> {
        self.ledger
            .reserve_memory(self.reservation, bytes, lifetime)?;
        Ok(MemoryLease::new(
            bytes,
            Arc::new(NativeMemoryLease {
                ledger: Arc::clone(&self.ledger),
                reservation: self.reservation,
                lifetime,
            }),
        ))
    }
}

struct NativeMemoryLease {
    ledger: Arc<NativeResourceLedger>,
    reservation: u64,
    lifetime: MemoryLifetime,
}

impl MemoryLeaseOwner for NativeMemoryLease {
    fn release(&self, bytes: u64) {
        if let Err(_error) = self
            .ledger
            .release_memory(self.reservation, bytes, self.lifetime)
        {
            self.ledger.invalid.store(true, Ordering::Release);
        }
    }
}

impl NativeResourceLedger {
    fn promised(&self, state: &LedgerState) -> Option<ResourceNeed> {
        let mut promised = state.total;
        promised.memory_bytes = promised
            .memory_bytes
            .checked_add(state.retained_memory_bytes)?
            .checked_add(state.pending_escape_bytes)?;
        for class in NativeResourceClass::ALL {
            if class == NativeResourceClass::BackgroundControl && !self.background_control_enabled
                || class == NativeResourceClass::MaintenanceControl
                    && !self.maintenance_control_enabled
            {
                continue;
            }
            promised = add(
                promised,
                floor_shortfall(self.plan.need(class), state.class(class).reserved),
            )?;
        }
        Some(promised)
    }

    /// Admit transport data after query execution. Protected class floors and
    /// existing retained/pending bytes remain in the same promised envelope.
    pub(super) fn reserve_response_memory(
        self: &Arc<Self>,
        bytes: u64,
    ) -> Result<MemoryLease, IndexError> {
        if bytes == 0 || bytes > self.plan.interactive.memory_bytes {
            return Err(IndexError::ResourceLimited);
        }
        if self.invalid.load(Ordering::Acquire) {
            return Err(invalid_ledger());
        }
        let mut state = self.state.lock().map_err(|_poison| invalid_ledger())?;
        if self.invalid.load(Ordering::Acquire) {
            return Err(invalid_ledger());
        }
        let extra = ResourceNeed {
            memory_bytes: bytes,
            ..ResourceNeed::default()
        };
        let promised = self
            .promised(&state)
            .and_then(|promised| add(promised, extra))
            .ok_or(IndexError::ResourceLimited)?;
        if !fits(promised, self.plan.capacity) {
            return Err(IndexError::ResourceLimited);
        }
        let reservation = state
            .next_reservation
            .checked_add(1)
            .ok_or(IndexError::ResourceLimited)?;
        let live = state
            .live_memory_bytes
            .checked_add(bytes)
            .ok_or(IndexError::ResourceLimited)?;
        let retained = state
            .retained_memory_bytes
            .checked_add(bytes)
            .ok_or(IndexError::ResourceLimited)?;
        if state.allocations.contains_key(&reservation) {
            self.invalid.store(true, Ordering::Release);
            return Err(invalid_ledger());
        }
        state.next_reservation = reservation;
        state.live_memory_bytes = live;
        state.retained_memory_bytes = retained;
        let _previous = state.allocations.insert(
            reservation,
            AllocationState {
                class: NativeResourceClass::Interactive,
                need: extra,
                active: false,
                temporary_bytes: 0,
                retained_bytes: bytes,
            },
        );
        drop(state);
        Ok(MemoryLease::new(
            bytes,
            Arc::new(NativeMemoryLease {
                ledger: Arc::clone(self),
                reservation,
                lifetime: MemoryLifetime::Retained,
            }),
        ))
    }

    fn reserve_memory(
        &self,
        reservation: u64,
        bytes: u64,
        lifetime: MemoryLifetime,
    ) -> Result<(), IndexError> {
        if bytes == 0 {
            return Err(IndexError::InvalidInput(
                "live memory reservation must be nonzero".into(),
            ));
        }
        if self.invalid.load(Ordering::Acquire) {
            return Err(invalid_ledger());
        }
        let mut state = self.state.lock().map_err(|_poison| invalid_ledger())?;
        if self.invalid.load(Ordering::Acquire) {
            return Err(invalid_ledger());
        }
        let mut allocation = state
            .allocations
            .get(&reservation)
            .copied()
            .filter(|allocation| allocation.active)
            .ok_or_else(|| IndexError::BackendViolation("allocation scope has ended".into()))?;
        let _bounded_live = allocation
            .temporary_bytes
            .checked_add(allocation.retained_bytes)
            .and_then(|live| live.checked_add(bytes))
            .filter(|live| *live <= allocation.need.memory_bytes)
            .ok_or(IndexError::ResourceLimited)?;
        let pending = match lifetime {
            MemoryLifetime::Temporary => {
                allocation.temporary_bytes = allocation
                    .temporary_bytes
                    .checked_add(bytes)
                    .ok_or(IndexError::ResourceLimited)?;
                state.pending_escape_bytes
            }
            MemoryLifetime::Retained => {
                allocation.retained_bytes = allocation
                    .retained_bytes
                    .checked_add(bytes)
                    .ok_or(IndexError::ResourceLimited)?;
                let next = state
                    .pending_escape_bytes
                    .checked_add(bytes)
                    .ok_or(IndexError::ResourceLimited)?;
                let extra = ResourceNeed {
                    memory_bytes: bytes,
                    ..ResourceNeed::default()
                };
                let promised = self
                    .promised(&state)
                    .and_then(|promised| add(promised, extra))
                    .ok_or(IndexError::ResourceLimited)?;
                if !fits(promised, self.plan.capacity) {
                    return Err(IndexError::ResourceLimited);
                }
                next
            }
        };
        let live_total = state
            .live_memory_bytes
            .checked_add(bytes)
            .ok_or(IndexError::ResourceLimited)?;
        state.pending_escape_bytes = pending;
        state.live_memory_bytes = live_total;
        let _previous = state.allocations.insert(reservation, allocation);
        drop(state);
        Ok(())
    }

    fn release_memory(
        &self,
        reservation: u64,
        bytes: u64,
        lifetime: MemoryLifetime,
    ) -> Result<(), IndexError> {
        // Cleanup still runs after another failure poisoned admission.
        let mut state = self.state.lock().map_err(|_poison| invalid_ledger())?;
        let mut allocation = state
            .allocations
            .get(&reservation)
            .copied()
            .ok_or_else(invalid_ledger)?;
        let (temporary, retained) = match lifetime {
            MemoryLifetime::Temporary => (
                allocation
                    .temporary_bytes
                    .checked_sub(bytes)
                    .ok_or_else(invalid_ledger)?,
                allocation.retained_bytes,
            ),
            MemoryLifetime::Retained => (
                allocation.temporary_bytes,
                allocation
                    .retained_bytes
                    .checked_sub(bytes)
                    .ok_or_else(invalid_ledger)?,
            ),
        };
        let live_total = state
            .live_memory_bytes
            .checked_sub(bytes)
            .ok_or_else(invalid_ledger)?;
        let pending = if allocation.active && lifetime == MemoryLifetime::Retained {
            state
                .pending_escape_bytes
                .checked_sub(bytes)
                .ok_or_else(invalid_ledger)?
        } else {
            state.pending_escape_bytes
        };
        let detached = if allocation.active {
            state.retained_memory_bytes
        } else {
            state
                .retained_memory_bytes
                .checked_sub(bytes)
                .ok_or_else(invalid_ledger)?
        };
        allocation.temporary_bytes = temporary;
        allocation.retained_bytes = retained;
        state.live_memory_bytes = live_total;
        state.pending_escape_bytes = pending;
        state.retained_memory_bytes = detached;
        if !allocation.active && temporary == 0 && retained == 0 {
            let _removed = state.allocations.remove(&reservation);
        } else {
            let _previous = state.allocations.insert(reservation, allocation);
        }
        drop(state);
        Ok(())
    }

    fn finish_execution(
        &self,
        class: NativeResourceClass,
        token: &NativeResourceToken,
    ) -> Result<(), IndexError> {
        let mut state = self.state.lock().map_err(|_poison| invalid_ledger())?;
        let mut allocation = state
            .allocations
            .get(&token.reservation)
            .copied()
            .filter(|allocation| {
                allocation.active
                    && allocation.class == class
                    && token.class == class
                    && allocation.need == token.need
            })
            .ok_or_else(invalid_ledger)?;
        let total = subtract(state.total, token.need).ok_or_else(invalid_ledger)?;
        let current_class = *state.class(class);
        let class_reserved =
            subtract(current_class.reserved, token.need).ok_or_else(invalid_ledger)?;
        let active = current_class
            .active
            .checked_sub(1)
            .ok_or_else(invalid_ledger)?;
        let pending = state
            .pending_escape_bytes
            .checked_sub(allocation.retained_bytes)
            .ok_or_else(invalid_ledger)?;
        let live = allocation
            .temporary_bytes
            .checked_add(allocation.retained_bytes)
            .ok_or_else(invalid_ledger)?;
        let retained = state
            .retained_memory_bytes
            .checked_add(live)
            .ok_or_else(invalid_ledger)?;
        state.total = total;
        state.pending_escape_bytes = pending;
        state.retained_memory_bytes = retained;
        *state.class_mut(class) = NativeClassSnapshot {
            active,
            reserved: class_reserved,
        };
        allocation.active = false;
        if live == 0 {
            let _removed = state.allocations.remove(&token.reservation);
        } else {
            let _previous = state.allocations.insert(token.reservation, allocation);
        }
        // A temporary lease cannot legitimately escape the execution stage.
        // Keep its bytes charged for cleanup and stop all new admission.
        if allocation.temporary_bytes != 0
            || self
                .promised(&state)
                .is_none_or(|promised| !fits(promised, self.plan.capacity))
        {
            self.invalid.store(true, Ordering::Release);
            drop(state);
            return Err(invalid_ledger());
        }
        drop(state);
        Ok(())
    }
}

fn invalid_ledger() -> IndexError {
    IndexError::BackendViolation("native resource ledger invariant failed".into())
}

fn fits(need: ResourceNeed, cap: ResourceNeed) -> bool {
    need.cpu_units <= cap.cpu_units
        && need.io_bytes <= cap.io_bytes
        && need.memory_bytes <= cap.memory_bytes
        && need.provider_units <= cap.provider_units
}

fn add(a: ResourceNeed, b: ResourceNeed) -> Option<ResourceNeed> {
    Some(ResourceNeed {
        cpu_units: a.cpu_units.checked_add(b.cpu_units)?,
        io_bytes: a.io_bytes.checked_add(b.io_bytes)?,
        memory_bytes: a.memory_bytes.checked_add(b.memory_bytes)?,
        provider_units: a.provider_units.checked_add(b.provider_units)?,
    })
}

fn subtract(a: ResourceNeed, b: ResourceNeed) -> Option<ResourceNeed> {
    Some(ResourceNeed {
        cpu_units: a.cpu_units.checked_sub(b.cpu_units)?,
        io_bytes: a.io_bytes.checked_sub(b.io_bytes)?,
        memory_bytes: a.memory_bytes.checked_sub(b.memory_bytes)?,
        provider_units: a.provider_units.checked_sub(b.provider_units)?,
    })
}

fn floor_shortfall(floor: ResourceNeed, reserved: ResourceNeed) -> ResourceNeed {
    ResourceNeed {
        cpu_units: floor.cpu_units.saturating_sub(reserved.cpu_units),
        io_bytes: floor.io_bytes.saturating_sub(reserved.io_bytes),
        memory_bytes: floor.memory_bytes.saturating_sub(reserved.memory_bytes),
        provider_units: floor.provider_units.saturating_sub(reserved.provider_units),
    }
}

#[cfg(test)]
mod tests {
    use quanta_index_core::index::effort::ReservationGuard;

    use super::*;

    macro_rules! check {
        ($condition:expr) => {{
            if $condition {
                Ok::<(), IndexError>(())
            } else {
                Err(IndexError::BackendViolation(format!(
                    "native resource check failed: {}",
                    stringify!($condition)
                )))
            }
        }};
        ($condition:expr, $context:literal) => {{
            if $condition {
                Ok::<(), IndexError>(())
            } else {
                Err(IndexError::BackendViolation(format!(
                    "native resource check failed: {}",
                    $context
                )))
            }
        }};
    }

    macro_rules! check_eq {
        ($actual:expr, $expected:expr) => {{
            let actual = &$actual;
            let expected = &$expected;
            if actual == expected {
                Ok::<(), IndexError>(())
            } else {
                Err(IndexError::BackendViolation(format!(
                    "native resource check {} == {}: actual={actual:?}, expected={expected:?}",
                    stringify!($actual),
                    stringify!($expected)
                )))
            }
        }};
        ($actual:expr, $expected:expr, $context:literal) => {{
            let actual = &$actual;
            let expected = &$expected;
            if actual == expected {
                Ok::<(), IndexError>(())
            } else {
                Err(IndexError::BackendViolation(format!(
                    "native resource check {}: actual={actual:?}, expected={expected:?}",
                    $context
                )))
            }
        }};
    }

    fn plan() -> NativeResourcePlan {
        let unit = ResourceNeed {
            cpu_units: 1,
            io_bytes: 1024,
            memory_bytes: 2048,
            provider_units: 1,
        };
        NativeResourcePlan {
            capacity: ResourceNeed {
                cpu_units: 4,
                io_bytes: 4096,
                memory_bytes: 8192,
                provider_units: 4,
            },
            control: unit,
            interactive: unit,
            core: unit,
            enrichment: unit,
        }
    }

    fn capacity_for_slots(
        mut configured: NativeResourcePlan,
        slots: u64,
    ) -> Result<NativeResourcePlan, IndexError> {
        configured.capacity = ResourceNeed {
            cpu_units: slots,
            io_bytes: 1024_u64
                .checked_mul(slots)
                .ok_or(IndexError::ResourceLimited)?,
            memory_bytes: 2048_u64
                .checked_mul(slots)
                .ok_or(IndexError::ResourceLimited)?,
            provider_units: slots,
        };
        Ok(configured)
    }

    #[test]
    fn response_frame_floor_denial_and_data_only_last_release() -> Result<(), IndexError> {
        let floors = NativeResourceLedger::new(plan())?;
        check!(matches!(
            floors.reserve_response_memory(1),
            Err(IndexError::ResourceLimited)
        ))?;
        check_eq!(floors.snapshot()?.live_memory_bytes, 0)?;
        let ledger = NativeResourceLedger::new(capacity_for_slots(plan(), 6)?)?;
        let payload = ledger.reserve_response_memory(2000)?;
        let frame = ledger.reserve_response_memory(2000)?;
        check_eq!(ledger.snapshot()?.live_memory_bytes, 4000)?;
        check_eq!(ledger.snapshot()?.retained_memory_bytes, 4000)?;
        check_eq!(ledger.snapshot()?.reserved, ResourceNeed::default())?;
        check_eq!(ledger.snapshot()?.pending_escape_bytes, 0)?;
        check!(matches!(
            ledger.reserve_response_memory(97),
            Err(IndexError::ResourceLimited)
        ))?;
        let shared = frame.clone();
        drop(frame);
        drop(payload);
        check_eq!(ledger.snapshot()?.retained_memory_bytes, 2000)?;
        drop(shared);
        check_eq!(ledger.snapshot()?.live_memory_bytes, 0)?;
        Ok(())
    }

    #[test]
    fn response_frame_maximum_wire_shape_admits_exact_backing()
    -> Result<(), Box<dyn std::error::Error>> {
        let maximum = quanta_index_ipc::MAX_FRAME_BODY_BYTES;
        let total = maximum.checked_add(4).ok_or(IndexError::ResourceLimited)?;
        let payload_len = maximum.checked_sub(5).ok_or(IndexError::ResourceLimited)?;
        let total_u64 = u64::try_from(total)?;
        let mut configured = plan();
        configured.interactive.memory_bytes = total_u64;
        configured.capacity.memory_bytes = total_u64
            .checked_mul(3)
            .and_then(|bytes| bytes.checked_add(6144))
            .ok_or(IndexError::ResourceLimited)?;
        let ledger = NativeResourceLedger::new(configured)?;
        let payload_memory = ledger.reserve_response_memory(u64::try_from(payload_len)?)?;
        let payload = "x".repeat(payload_len);
        let budget =
            quanta_index_core::RequestBudgetV1::for_duration(std::time::Duration::from_secs(5));
        let frame = quanta_index_ipc::encode_response_admitted(&payload, &budget, |bytes| {
            ledger
                .reserve_response_memory(bytes)
                .map(Some)
                .map_err(|_error| quanta_index_ipc::IpcError::FrameMemoryLimited { bytes })
        })?;
        check_eq!(frame.len(), total)?;
        // Independent canonical CBOR text-header golden at the wire cap.
        let expected_frame_length = u32::try_from(maximum)?.to_le_bytes();
        let expected_text_length = u32::try_from(payload_len)?.to_be_bytes();
        check_eq!(frame.get(..4), Some(expected_frame_length.as_slice()))?;
        check_eq!(frame.get(4), Some(&0x7a))?;
        check_eq!(frame.get(5..9), Some(expected_text_length.as_slice()))?;
        check_eq!(frame.last(), Some(&b'x'))?;
        check_eq!(
            ledger.snapshot()?.live_memory_bytes,
            u64::try_from(
                payload_len
                    .checked_add(total)
                    .ok_or(IndexError::ResourceLimited)?
            )?
        )?;
        check_eq!(ledger.snapshot()?.reserved, ResourceNeed::default())?;
        drop(frame);
        drop(payload);
        drop(payload_memory);
        check_eq!(ledger.snapshot()?.live_memory_bytes, 0)?;
        Ok(())
    }

    #[test]
    fn response_frame_cancel_inside_count_and_encoding_returns_charge()
    -> Result<(), Box<dyn std::error::Error>> {
        struct CancelDuring {
            pass: std::sync::atomic::AtomicUsize,
            phase: usize,
            cancel: quanta_index_core::CancelHandleV1,
        }
        impl serde::Serialize for CancelDuring {
            fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
                use serde::ser::SerializeSeq;
                let phase = self.pass.fetch_add(1, Ordering::SeqCst).saturating_add(1);
                let mut seq = serializer.serialize_seq(Some(2))?;
                seq.serialize_element(&1_u64)?;
                if phase == self.phase {
                    self.cancel.cancel();
                }
                seq.serialize_element(&2_u64)?;
                seq.end()
            }
        }
        for phase in [1, 2] {
            let ledger = NativeResourceLedger::new(capacity_for_slots(plan(), 6)?)?;
            let budget =
                quanta_index_core::RequestBudgetV1::for_duration(std::time::Duration::from_secs(5));
            let value = CancelDuring {
                pass: std::sync::atomic::AtomicUsize::new(0),
                phase,
                cancel: budget.cancel_handle(),
            };
            let mut admissions = 0;
            let outcome = quanta_index_ipc::encode_response_admitted(&value, &budget, |bytes| {
                admissions += 1;
                ledger
                    .reserve_response_memory(bytes)
                    .map(Some)
                    .map_err(|_error| quanta_index_ipc::IpcError::FrameMemoryLimited { bytes })
            });
            check!(
                matches!(outcome, Err(quanta_index_ipc::IpcError::Io(error)) if error.kind() == std::io::ErrorKind::Other)
            )?;
            check_eq!(admissions, phase.saturating_sub(1))?;
            check_eq!(ledger.snapshot()?.live_memory_bytes, 0)?;
            check_eq!(ledger.snapshot()?.reserved, ResourceNeed::default())?;
        }
        Ok(())
    }

    #[test]
    fn response_frame_canonical_bytes_and_partial_error_return_real_ledger_charge()
    -> Result<(), Box<dyn std::error::Error>> {
        struct Changing(std::sync::atomic::AtomicUsize);
        impl serde::Serialize for Changing {
            fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
                serializer.serialize_u64(if self.0.fetch_add(1, Ordering::SeqCst) == 0 {
                    1
                } else {
                    1000
                })
            }
        }
        let ledger = NativeResourceLedger::new(capacity_for_slots(plan(), 6)?)?;
        let budget =
            quanta_index_core::RequestBudgetV1::for_duration(std::time::Duration::from_secs(5));
        let admit = |bytes| {
            ledger
                .reserve_response_memory(bytes)
                .map(Some)
                .map_err(|_error| quanta_index_ipc::IpcError::FrameMemoryLimited { bytes })
        };
        let frame =
            quanta_index_ipc::encode_response_admitted(&vec![1_u64, 23, 24, 1000], &budget, admit)?;
        // Fixed CBOR array golden, with its four-byte little-endian length.
        check_eq!(&*frame, &[8, 0, 0, 0, 0x84, 1, 23, 0x18, 24, 0x19, 3, 0xe8])?;
        check_eq!(
            ledger.snapshot()?.retained_memory_bytes,
            u64::try_from(frame.len())?
        )?;
        drop(frame);
        check_eq!(ledger.snapshot()?.live_memory_bytes, 0)?;
        let changed = Changing(std::sync::atomic::AtomicUsize::new(0));
        check!(quanta_index_ipc::encode_response_admitted(&changed, &budget, admit).is_err())?;
        check_eq!(ledger.snapshot()?.live_memory_bytes, 0)?;
        check!(
            quanta_index_ipc::encode_response_admitted(&1_u64, &budget, |bytes| {
                let memory = admit(bytes)?;
                budget.cancel_handle().cancel();
                Ok(memory)
            })
            .is_err()
        )?;
        check_eq!(ledger.snapshot()?.live_memory_bytes, 0)?;
        Ok(())
    }

    #[test]
    fn maintenance_and_owned_pump_share_one_protected_ledger() -> Result<(), IndexError> {
        let plan = capacity_for_slots(plan(), 6)?;
        plan.validate_maintenance(true)?;
        let ledger = NativeResourceLedger::with_maintenance(plan, true, true)?;
        let maintenance = ReservationGuard::try_acquire(
            ledger.scheduler(NativeResourceClass::MaintenanceControl),
            plan.control,
        )
        .ok_or(IndexError::ResourceLimited)?;
        let foreground = ReservationGuard::try_acquire(
            ledger.scheduler(NativeResourceClass::Control),
            plan.control,
        )
        .ok_or(IndexError::ResourceLimited)?;
        let core =
            ReservationGuard::try_acquire(ledger.scheduler(NativeResourceClass::Core), plan.core)
                .ok_or(IndexError::ResourceLimited)?;
        check!(
            ReservationGuard::try_acquire(ledger.scheduler(NativeResourceClass::Core), plan.core,)
                .is_none(),
            "extra Core cannot consume the owned pump renewal floor"
        )?;
        let renewal = ReservationGuard::try_acquire(
            ledger.scheduler(NativeResourceClass::BackgroundControl),
            plan.control,
        )
        .ok_or(IndexError::ResourceLimited)?;
        let interactive = ReservationGuard::try_acquire(
            ledger.scheduler(NativeResourceClass::Interactive),
            plan.interactive,
        )
        .ok_or(IndexError::ResourceLimited)?;
        let enrichment = ReservationGuard::try_acquire(
            ledger.scheduler(NativeResourceClass::Enrichment),
            plan.enrichment,
        )
        .ok_or(IndexError::ResourceLimited)?;
        let snapshot = ledger.snapshot()?;
        check_eq!(snapshot.reserved, plan.capacity)?;
        check_eq!(snapshot.maintenance_control.active, 1)?;
        check_eq!(snapshot.background_control.active, 1)?;
        drop((
            maintenance,
            foreground,
            core,
            renewal,
            interactive,
            enrichment,
        ));
        check_eq!(ledger.snapshot()?.reserved, ResourceNeed::default())?;

        let disabled = NativeResourceLedger::with_maintenance(plan, true, false)?;
        check!(
            ReservationGuard::try_acquire(
                disabled.scheduler(NativeResourceClass::MaintenanceControl),
                plan.control,
            )
            .is_none()
        )?;
        Ok(())
    }

    #[test]
    fn parallel_maintenance_requires_combined_seven_slot_floor() -> Result<(), IndexError> {
        let six = capacity_for_slots(plan(), 6)?;
        six.validate_parallel_owned_pump(2, ResourceNeed::default())?;
        check!(matches!(
            six.validate_parallel_maintenance(2, ResourceNeed::default()),
            Err(IndexError::InvalidInput(_))
        ))?;
        let seven = capacity_for_slots(plan(), 7)?;
        seven.validate_parallel_maintenance(2, ResourceNeed::default())?;
        let _ledger = NativeResourceLedger::with_maintenance(seven, true, true)?;
        Ok(())
    }

    #[test]
    fn parallel_provider_capacity_uses_frozen_builder_demand() -> Result<(), IndexError> {
        let mut configured = plan();
        configured.capacity.cpu_units = 6;
        configured.capacity.io_bytes = configured
            .enrichment
            .io_bytes
            .checked_mul(6)
            .ok_or(IndexError::ResourceLimited)?;
        configured.capacity.memory_bytes = configured
            .enrichment
            .memory_bytes
            .checked_mul(6)
            .ok_or(IndexError::ResourceLimited)?;
        configured.capacity.provider_units = 5;
        configured.validate_parallel_owned_pump(2, ResourceNeed::default())?;
        check!(matches!(
            configured.validate_parallel_owned_pump(2, configured.enrichment),
            Err(IndexError::InvalidInput(_))
        ))?;
        Ok(())
    }

    #[test]
    fn core_reservation_uses_declared_provider_demand_below_envelope() -> Result<(), IndexError> {
        let configured = plan();
        let ledger = NativeResourceLedger::new(configured)?;
        let scheduler = ledger.scheduler(NativeResourceClass::Core);
        let need = ResourceNeed {
            provider_units: 0,
            ..configured.core
        };
        let held =
            ReservationGuard::try_acquire(scheduler, need).ok_or(IndexError::ResourceLimited)?;
        let snapshot = ledger.snapshot()?;
        check_eq!(snapshot.core.reserved.provider_units, 0)?;
        check_eq!(snapshot.reserved.provider_units, 0)?;
        drop(held);
        check_eq!(ledger.snapshot()?.reserved, ResourceNeed::default())?;
        Ok(())
    }

    #[test]
    fn owned_control_preserves_foreground_floor_under_full_execution() -> Result<(), IndexError> {
        check!(matches!(
            plan().validate_owned_pump(),
            Err(IndexError::InvalidInput(_))
        ))?;
        let mut owned = plan();
        owned.capacity.cpu_units = 5;
        owned.capacity.provider_units = 5;
        owned.capacity.io_bytes = owned
            .control
            .io_bytes
            .checked_mul(5)
            .ok_or(IndexError::ResourceLimited)?;
        owned.capacity.memory_bytes = owned
            .control
            .memory_bytes
            .checked_mul(5)
            .ok_or(IndexError::ResourceLimited)?;
        let ledger = NativeResourceLedger::with_background_control(owned, true)?;
        let background_scheduler = ledger.scheduler(NativeResourceClass::BackgroundControl);
        let background =
            ReservationGuard::try_acquire(Arc::clone(&background_scheduler), owned.control)
                .ok_or(IndexError::ResourceLimited)?;
        check!(ReservationGuard::try_acquire(background_scheduler, owned.control).is_none())?;
        let core =
            ReservationGuard::try_acquire(ledger.scheduler(NativeResourceClass::Core), owned.core)
                .ok_or(IndexError::ResourceLimited)?;
        let enrichment = ReservationGuard::try_acquire(
            ledger.scheduler(NativeResourceClass::Enrichment),
            owned.enrichment,
        )
        .ok_or(IndexError::ResourceLimited)?;
        let foreground = ReservationGuard::try_acquire(
            ledger.scheduler(NativeResourceClass::Control),
            owned.control,
        )
        .ok_or(IndexError::ResourceLimited)?;
        let interactive = ReservationGuard::try_acquire(
            ledger.scheduler(NativeResourceClass::Interactive),
            owned.interactive,
        )
        .ok_or(IndexError::ResourceLimited)?;
        let snapshot = ledger.snapshot()?;
        check_eq!(snapshot.reserved.cpu_units, 5)?;
        check_eq!(snapshot.control.active, 1)?;
        check_eq!(snapshot.background_control.active, 1)?;
        drop((background, core, enrichment, foreground, interactive));
        check_eq!(ledger.snapshot()?.reserved, ResourceNeed::default())?;
        let manual = NativeResourceLedger::new(plan())?;
        check!(
            ReservationGuard::try_acquire(
                manual.scheduler(NativeResourceClass::BackgroundControl),
                plan().control,
            )
            .is_none()
        )?;
        Ok(())
    }

    struct ExpectedUnwind;

    fn intentional_unwind<T>(held: T) {
        // The owned reservation must unwind on this frame.
        let _held = held;
        std::panic::panic_any(ExpectedUnwind);
    }

    #[test]
    fn exhaustion_and_unwind_release_exactly_once() -> Result<(), IndexError> {
        let ledger = NativeResourceLedger::new(plan())?;
        let unit = plan().core;
        let core = ledger.scheduler(NativeResourceClass::Core);
        let held = ReservationGuard::try_acquire(Arc::clone(&core), unit)
            .ok_or(IndexError::ResourceLimited)?;
        check!(
            ReservationGuard::try_acquire(Arc::clone(&core), unit).is_none(),
            "second Core reservation accepted while held"
        )?;
        check_eq!(
            ledger.snapshot()?.reserved.cpu_units,
            1,
            "Core reservation held before unwind"
        )?;
        let unwind = std::panic::catch_unwind(std::panic::AssertUnwindSafe(move || {
            intentional_unwind(held);
        }));
        check!(
            unwind.is_err_and(|panic| panic.is::<ExpectedUnwind>()),
            "intentional unwind marker missing"
        )?;
        check_eq!(
            ledger.snapshot()?.reserved,
            ResourceNeed::default(),
            "reservation released after unwind"
        )?;
        let again = ReservationGuard::try_acquire(core, unit).ok_or(IndexError::ResourceLimited)?;
        drop(again);
        Ok(())
    }

    #[test]
    fn invalid_capacity_and_underflow_fail_closed() -> Result<(), IndexError> {
        let mut invalid = plan();
        invalid.core.io_bytes = invalid
            .capacity
            .io_bytes
            .checked_add(1)
            .ok_or(IndexError::ResourceLimited)?;
        check!(matches!(
            NativeResourceLedger::new(invalid),
            Err(IndexError::InvalidInput(_))
        ))?;
        let mut overcommitted = plan();
        overcommitted.capacity.cpu_units = 3;
        check!(matches!(
            NativeResourceLedger::new(overcommitted),
            Err(IndexError::InvalidInput(_))
        ))?;
        let ledger = NativeResourceLedger::new(plan())?;
        let core = ledger.scheduler(NativeResourceClass::Core);
        check!(
            ReservationGuard::try_acquire(Arc::clone(&core), ResourceNeed::default()).is_none()
        )?;
        check_eq!(ledger.snapshot()?.reserved, ResourceNeed::default())?;
        core.release(NativeResourceToken {
            class: NativeResourceClass::Core,
            need: plan().core,
            reservation: 0,
        });
        check!(matches!(
            ledger.snapshot(),
            Err(IndexError::BackendViolation(_))
        ))?;
        check!(ReservationGuard::try_acquire(core, plan().core).is_none())?;
        Ok(())
    }

    #[test]
    fn four_classes_keep_protected_floor_and_borrow_only_spare() -> Result<(), IndexError> {
        let one = plan().core;
        let plan = plan();
        let ledger = NativeResourceLedger::new(plan)?;
        let interactive_scheduler = ledger.scheduler(NativeResourceClass::Interactive);
        let control_scheduler = ledger.scheduler(NativeResourceClass::Control);
        let core_scheduler = ledger.scheduler(NativeResourceClass::Core);
        let enrichment_scheduler = ledger.scheduler(NativeResourceClass::Enrichment);
        let interactive =
            ReservationGuard::try_acquire(Arc::clone(&interactive_scheduler), plan.interactive)
                .ok_or(IndexError::ResourceLimited)?;
        check!(ReservationGuard::try_acquire(Arc::clone(&interactive_scheduler), one).is_none())?;
        let core = ReservationGuard::try_acquire(Arc::clone(&core_scheduler), plan.core)
            .ok_or(IndexError::ResourceLimited)?;
        let enrichment =
            ReservationGuard::try_acquire(Arc::clone(&enrichment_scheduler), plan.enrichment)
                .ok_or(IndexError::ResourceLimited)?;
        let control = ReservationGuard::try_acquire(control_scheduler, plan.control)
            .ok_or(IndexError::ResourceLimited)?;
        let snapshot = ledger.snapshot()?;
        check_eq!(snapshot.reserved.cpu_units, 4)?;
        check_eq!(snapshot.core.active, 1)?;
        check_eq!(snapshot.control.active, 1)?;
        check!(ReservationGuard::try_acquire(Arc::clone(&interactive_scheduler), one).is_none())?;
        drop(core);
        // Releasing Core restores its protected slot; Interactive cannot
        // borrow it while Core is absent.
        check!(ReservationGuard::try_acquire(Arc::clone(&interactive_scheduler), one).is_none())?;
        let core_again = ReservationGuard::try_acquire(core_scheduler, plan.core)
            .ok_or(IndexError::ResourceLimited)?;
        drop(core_again);
        drop(control);
        drop(interactive);
        drop(enrichment);
        check_eq!(ledger.snapshot()?.reserved, ResourceNeed::default())?;

        let mut spare = plan;
        spare.capacity.cpu_units = spare
            .capacity
            .cpu_units
            .checked_add(1)
            .ok_or(IndexError::ResourceLimited)?;
        spare.capacity.io_bytes = spare
            .capacity
            .io_bytes
            .checked_add(one.io_bytes)
            .ok_or(IndexError::ResourceLimited)?;
        spare.capacity.memory_bytes = spare
            .capacity
            .memory_bytes
            .checked_add(one.memory_bytes)
            .ok_or(IndexError::ResourceLimited)?;
        spare.capacity.provider_units = spare
            .capacity
            .provider_units
            .checked_add(one.provider_units)
            .ok_or(IndexError::ResourceLimited)?;
        let ledger = NativeResourceLedger::new(spare)?;
        let interactive = ledger.scheduler(NativeResourceClass::Interactive);
        let first = ReservationGuard::try_acquire(Arc::clone(&interactive), one)
            .ok_or(IndexError::ResourceLimited)?;
        let borrowed =
            ReservationGuard::try_acquire(interactive, one).ok_or(IndexError::ResourceLimited)?;
        check_eq!(ledger.snapshot()?.reserved.cpu_units, 2)?;
        drop(first);
        drop(borrowed);
        Ok(())
    }

    #[test]
    fn interactive_reserves_actual_need_within_its_envelope() -> Result<(), IndexError> {
        let plan = plan();
        let ledger = NativeResourceLedger::new(plan)?;
        let lexical = ResourceNeed {
            provider_units: 0,
            ..plan.interactive
        };
        let held = ReservationGuard::try_acquire(
            ledger.scheduler(NativeResourceClass::Interactive),
            lexical,
        )
        .ok_or(IndexError::ResourceLimited)?;
        let snapshot = ledger.snapshot()?;
        check_eq!(snapshot.interactive.active, 1)?;
        check_eq!(snapshot.interactive.reserved, lexical)?;
        check_eq!(snapshot.reserved, lexical)?;
        check_eq!(snapshot.reserved.provider_units, 0)?;
        drop(held);
        check_eq!(ledger.snapshot()?.reserved, ResourceNeed::default())?;
        Ok(())
    }

    #[test]
    fn refinement_reserves_actual_need_and_preserves_foreground_floor() -> Result<(), IndexError> {
        let plan = plan();
        let ledger = NativeResourceLedger::new(plan)?;
        let refinement = ResourceNeed {
            io_bytes: 512,
            memory_bytes: 1024,
            ..plan.enrichment
        };
        let scheduler = ledger.scheduler(NativeResourceClass::Enrichment);
        let held = ReservationGuard::try_acquire(Arc::clone(&scheduler), refinement)
            .ok_or(IndexError::ResourceLimited)?;
        check_eq!(ledger.snapshot()?.enrichment.reserved, refinement)?;
        check!(ReservationGuard::try_acquire(scheduler, refinement).is_none())?;
        let foreground = ReservationGuard::try_acquire(
            ledger.scheduler(NativeResourceClass::Interactive),
            plan.interactive,
        )
        .ok_or(IndexError::ResourceLimited)?;
        check_eq!(ledger.snapshot()?.interactive.active, 1)?;
        drop((held, foreground));
        check_eq!(ledger.snapshot()?.reserved, ResourceNeed::default())?;
        Ok(())
    }

    #[test]
    fn refinement_rejects_zero_and_over_envelope_before_reservation() -> Result<(), IndexError> {
        let plan = plan();
        let ledger = NativeResourceLedger::new(plan)?;
        let scheduler = ledger.scheduler(NativeResourceClass::Enrichment);
        for need in [
            ResourceNeed::default(),
            ResourceNeed {
                cpu_units: 0,
                ..plan.enrichment
            },
            ResourceNeed {
                io_bytes: 0,
                ..plan.enrichment
            },
            ResourceNeed {
                memory_bytes: 0,
                ..plan.enrichment
            },
            ResourceNeed {
                provider_units: 2,
                ..plan.enrichment
            },
        ] {
            check!(ReservationGuard::try_acquire(Arc::clone(&scheduler), need).is_none())?;
            check_eq!(ledger.snapshot()?.reserved, ResourceNeed::default())?;
        }
        Ok(())
    }

    #[test]
    fn active_interactive_preserves_its_unused_provider_floor() -> Result<(), IndexError> {
        let mut plan = plan();
        // There is spare CPU, IO, and memory for one more Control operation,
        // but no provider capacity beyond the four protected class floors.
        plan.capacity.cpu_units = plan
            .capacity
            .cpu_units
            .checked_add(1)
            .ok_or(IndexError::ResourceLimited)?;
        plan.capacity.io_bytes = plan
            .capacity
            .io_bytes
            .checked_add(plan.control.io_bytes)
            .ok_or(IndexError::ResourceLimited)?;
        plan.capacity.memory_bytes = plan
            .capacity
            .memory_bytes
            .checked_add(plan.control.memory_bytes)
            .ok_or(IndexError::ResourceLimited)?;
        let ledger = NativeResourceLedger::new(plan)?;
        let lexical = ResourceNeed {
            provider_units: 0,
            ..plan.interactive
        };
        let interactive = ReservationGuard::try_acquire(
            ledger.scheduler(NativeResourceClass::Interactive),
            lexical,
        )
        .ok_or(IndexError::ResourceLimited)?;
        let control_scheduler = ledger.scheduler(NativeResourceClass::Control);
        let control = ReservationGuard::try_acquire(Arc::clone(&control_scheduler), plan.control)
            .ok_or(IndexError::ResourceLimited)?;
        let core =
            ReservationGuard::try_acquire(ledger.scheduler(NativeResourceClass::Core), plan.core)
                .ok_or(IndexError::ResourceLimited)?;
        let enrichment = ReservationGuard::try_acquire(
            ledger.scheduler(NativeResourceClass::Enrichment),
            plan.enrichment,
        )
        .ok_or(IndexError::ResourceLimited)?;
        let before = ledger.snapshot()?;
        check_eq!(before.reserved.provider_units, 3)?;
        check_eq!(before.interactive.active, 1)?;
        check!(ReservationGuard::try_acquire(control_scheduler, plan.control).is_none())?;
        check_eq!(ledger.snapshot()?, before)?;
        drop((interactive, control, core, enrichment));
        check_eq!(ledger.snapshot()?.reserved, ResourceNeed::default())?;
        Ok(())
    }

    #[test]
    fn interactive_rejects_zero_and_over_envelope() -> Result<(), IndexError> {
        let plan = plan();
        let ledger = NativeResourceLedger::new(plan)?;
        let scheduler = ledger.scheduler(NativeResourceClass::Interactive);
        let over_envelope = ResourceNeed {
            provider_units: 2,
            ..plan.interactive
        };
        let missing_cpu = ResourceNeed {
            cpu_units: 0,
            ..plan.interactive
        };
        let missing_io = ResourceNeed {
            io_bytes: 0,
            ..plan.interactive
        };
        let missing_memory = ResourceNeed {
            memory_bytes: 0,
            ..plan.interactive
        };
        for need in [
            ResourceNeed::default(),
            over_envelope,
            missing_cpu,
            missing_io,
            missing_memory,
        ] {
            check!(ReservationGuard::try_acquire(Arc::clone(&scheduler), need).is_none())?;
            check_eq!(ledger.snapshot()?.reserved, ResourceNeed::default())?;
        }
        Ok(())
    }

    #[test]
    fn control_class_still_requires_its_exact_need() -> Result<(), IndexError> {
        let plan = plan();
        let ledger = NativeResourceLedger::new(plan)?;
        let scheduler = ledger.scheduler(NativeResourceClass::Control);
        let reduced = ResourceNeed {
            provider_units: 0,
            ..plan.control
        };
        check!(ReservationGuard::try_acquire(Arc::clone(&scheduler), reduced).is_none())?;
        check_eq!(ledger.snapshot()?.reserved, ResourceNeed::default())?;
        let held = ReservationGuard::try_acquire(scheduler, plan.control)
            .ok_or(IndexError::ResourceLimited)?;
        check_eq!(ledger.snapshot()?.reserved, plan.control)?;
        drop(held);
        check_eq!(ledger.snapshot()?.reserved, ResourceNeed::default())?;
        Ok(())
    }

    #[test]
    fn retained_memory_refuses_escape_without_future_floor_headroom() -> Result<(), IndexError> {
        let configured = plan();
        let ledger = NativeResourceLedger::new(configured)?;
        let guard = ReservationGuard::try_acquire(
            ledger.scheduler(NativeResourceClass::Control),
            configured.control,
        )
        .ok_or(IndexError::ResourceLimited)?;
        let scope = guard.memory_scope().ok_or_else(invalid_ledger)?;
        let before = ledger.snapshot()?;
        check!(matches!(
            scope.reserve(1, MemoryLifetime::Retained),
            Err(IndexError::ResourceLimited)
        ))?;
        check_eq!(ledger.snapshot()?, before)?;
        // Temporary work uses its already admitted envelope, with no extra
        // resident charge or prospective commitment to another class floor.
        let temporary =
            scope.reserve(configured.control.memory_bytes, MemoryLifetime::Temporary)?;
        check_eq!(ledger.snapshot()?.reserved, configured.control)?;
        check_eq!(ledger.snapshot()?.pending_escape_bytes, 0)?;
        check!(matches!(
            scope.reserve(1, MemoryLifetime::Temporary),
            Err(IndexError::ResourceLimited)
        ))?;
        drop(temporary);
        drop(guard);
        check_eq!(ledger.snapshot()?.live_memory_bytes, 0)?;
        Ok(())
    }

    #[test]
    fn retained_memory_detaches_without_retaining_execution_slots() -> Result<(), IndexError> {
        let mut configured = plan();
        configured.capacity.memory_bytes = configured
            .capacity
            .memory_bytes
            .checked_add(1)
            .ok_or(IndexError::ResourceLimited)?;
        let ledger = NativeResourceLedger::new(configured)?;
        let guard = ReservationGuard::try_acquire(
            ledger.scheduler(NativeResourceClass::Control),
            configured.control,
        )
        .ok_or(IndexError::ResourceLimited)?;
        let scope = guard.memory_scope().ok_or_else(invalid_ledger)?;
        let lease = scope.reserve(1, MemoryLifetime::Retained)?;
        let clone = lease.clone();
        check_eq!(ledger.snapshot()?.pending_escape_bytes, 1)?;
        check_eq!(ledger.snapshot()?.retained_memory_bytes, 0)?;
        drop(guard);
        let detached = ledger.snapshot()?;
        check_eq!(detached.reserved, ResourceNeed::default())?;
        check_eq!(detached.control.active, 0)?;
        check_eq!(detached.retained_memory_bytes, 1)?;
        check_eq!(detached.pending_escape_bytes, 0)?;
        check!(matches!(
            scope.reserve(1, MemoryLifetime::Retained),
            Err(IndexError::BackendViolation(_))
        ))?;
        let mut executions = Vec::new();
        for (class, need) in [
            (NativeResourceClass::Control, configured.control),
            (NativeResourceClass::Interactive, configured.interactive),
            (NativeResourceClass::Core, configured.core),
            (NativeResourceClass::Enrichment, configured.enrichment),
        ] {
            executions.push(
                ReservationGuard::try_acquire(ledger.scheduler(class), need)
                    .ok_or(IndexError::ResourceLimited)?,
            );
        }
        check_eq!(ledger.snapshot()?.reserved.memory_bytes, 8192)?;
        drop(lease);
        check_eq!(ledger.snapshot()?.retained_memory_bytes, 1)?;
        drop(clone);
        check_eq!(ledger.snapshot()?.retained_memory_bytes, 0)?;
        drop(executions);
        check_eq!(ledger.snapshot()?.live_memory_bytes, 0)?;
        check_eq!(ledger.snapshot()?.reserved, ResourceNeed::default())?;
        Ok(())
    }

    #[test]
    fn retained_memory_commitment_cannot_be_consumed_by_another_allocation()
    -> Result<(), IndexError> {
        let mut configured = plan();
        configured.capacity.cpu_units = 5;
        configured.capacity.memory_bytes = 8256;
        let ledger = NativeResourceLedger::new(configured)?;
        let scheduler = ledger.scheduler(NativeResourceClass::Interactive);
        let need = ResourceNeed {
            memory_bytes: 512,
            io_bytes: 512,
            provider_units: 0,
            ..configured.interactive
        };
        // Both requests fit inside the class memory floor. Each execution's
        // escaping result still needs its own future retained capacity.
        let first = ReservationGuard::try_acquire(Arc::clone(&scheduler), need)
            .ok_or(IndexError::ResourceLimited)?;
        let first_scope = first.memory_scope().ok_or_else(invalid_ledger)?;
        let lease = first_scope.reserve(64, MemoryLifetime::Retained)?;
        let second =
            ReservationGuard::try_acquire(scheduler, need).ok_or(IndexError::ResourceLimited)?;
        let second_scope = second.memory_scope().ok_or_else(invalid_ledger)?;
        let before = ledger.snapshot()?;
        check!(matches!(
            second_scope.reserve(1, MemoryLifetime::Retained),
            Err(IndexError::ResourceLimited)
        ))?;
        check_eq!(ledger.snapshot()?, before)?;
        drop(first);
        check_eq!(ledger.snapshot()?.retained_memory_bytes, 64)?;
        drop(lease);
        let second_lease = second_scope.reserve(1, MemoryLifetime::Retained)?;
        drop(second_lease);
        drop(second);
        check_eq!(ledger.snapshot()?.live_memory_bytes, 0)?;
        check_eq!(ledger.snapshot()?.pending_escape_bytes, 0)?;
        Ok(())
    }

    #[test]
    fn retained_memory_last_clone_drop_can_race_execution_detachment() -> Result<(), IndexError> {
        let mut configured = plan();
        configured.capacity.memory_bytes = 8256;
        let ledger = NativeResourceLedger::new(configured)?;
        let guard = ReservationGuard::try_acquire(
            ledger.scheduler(NativeResourceClass::Interactive),
            configured.interactive,
        )
        .ok_or(IndexError::ResourceLimited)?;
        let lease = guard
            .memory_scope()
            .ok_or_else(invalid_ledger)?
            .reserve(64, MemoryLifetime::Retained)?;
        let barrier = Arc::new(std::sync::Barrier::new(3));
        let guard_barrier = Arc::clone(&barrier);
        let release_execution = std::thread::spawn(move || {
            let _leader = guard_barrier.wait();
            drop(guard);
        });
        let clone = lease.clone();
        let lease_barrier = Arc::clone(&barrier);
        let release_allocation = std::thread::spawn(move || {
            let _leader = lease_barrier.wait();
            drop(lease);
            drop(clone);
        });
        let _leader = barrier.wait();
        release_execution
            .join()
            .map_err(|_panic| IndexError::Storage("execution release panicked".into()))?;
        release_allocation
            .join()
            .map_err(|_panic| IndexError::Storage("allocation release panicked".into()))?;
        let snapshot = ledger.snapshot()?;
        check_eq!(snapshot.reserved, ResourceNeed::default())?;
        check_eq!(snapshot.live_memory_bytes, 0)?;
        check_eq!(snapshot.retained_memory_bytes, 0)?;
        check_eq!(snapshot.pending_escape_bytes, 0)?;
        check!(
            ledger
                .state
                .lock()
                .map_err(|_poison| invalid_ledger())?
                .allocations
                .is_empty()
        )?;
        Ok(())
    }

    #[test]
    fn retained_memory_temporary_escape_fails_closed_but_releases_live_charge()
    -> Result<(), IndexError> {
        let ledger = NativeResourceLedger::new(plan())?;
        let guard = ReservationGuard::try_acquire(
            ledger.scheduler(NativeResourceClass::Control),
            plan().control,
        )
        .ok_or(IndexError::ResourceLimited)?;
        let lease = guard
            .memory_scope()
            .ok_or_else(invalid_ledger)?
            .reserve(1, MemoryLifetime::Temporary)?;
        drop(guard);
        check!(matches!(
            ledger.snapshot(),
            Err(IndexError::BackendViolation(_))
        ))?;
        check!(
            ReservationGuard::try_acquire(
                ledger.scheduler(NativeResourceClass::Control),
                plan().control,
            )
            .is_none()
        )?;
        drop(lease);
        let state = ledger.state.lock().map_err(|_poison| invalid_ledger())?;
        check_eq!(state.live_memory_bytes, 0)?;
        check_eq!(state.retained_memory_bytes, 0)?;
        check!(state.allocations.is_empty())?;
        Ok(())
    }

    fn object_memory_budget() -> Result<quanta_index_core::index::WorkBudget, IndexError> {
        let deadline = std::time::Instant::now()
            .checked_add(std::time::Duration::from_secs(5))
            .ok_or(IndexError::ResourceLimited)?;
        Ok(quanta_index_core::index::WorkBudget::new(
            quanta_index_core::index::WorkLimits {
                reads: 100,
                writes: 100,
                bytes: 1024 * 1024,
                mutations: 100,
            },
            deadline,
        ))
    }

    fn object_memory_root() -> Result<tempfile::TempDir, IndexError> {
        use std::os::unix::fs::PermissionsExt;
        let root = tempfile::tempdir().map_err(|error| IndexError::Storage(error.to_string()))?;
        std::fs::set_permissions(root.path(), std::fs::Permissions::from_mode(0o700))
            .map_err(|error| IndexError::Storage(error.to_string()))?;
        Ok(root)
    }

    #[test]
    fn retained_memory_real_object_clone_keeps_charge_after_execution() -> Result<(), IndexError> {
        use quanta_index_core::index::{StoredObject, ports::ImmutableObjectPort};
        let root = object_memory_root()?;
        let store = quanta_index_storage::FileObjectStore::open(root.path())?;
        let payload = [19_u8; 512];
        let mut work = object_memory_budget()?;
        let reference = store.put(&payload, &[], &mut work)?.reference;
        let bytes = StoredObject::allocation_bytes(payload.len(), 0)?;
        let mut configured = plan();
        configured.capacity.memory_bytes = configured
            .capacity
            .memory_bytes
            .checked_add(bytes)
            .ok_or(IndexError::ResourceLimited)?;
        let ledger = NativeResourceLedger::new(configured)?;
        let guard = ReservationGuard::try_acquire(
            ledger.scheduler(NativeResourceClass::Interactive),
            configured.interactive,
        )
        .ok_or(IndexError::ResourceLimited)?;
        work.bind_memory_scope(guard.memory_scope());
        let stored = store.get(reference, &mut work)?;
        let clone = stored.clone();
        let registration = stored.registration.clone();
        check_eq!(stored.payload.as_slice(), payload.as_slice())?;
        check_eq!(stored.memory_bytes(), Some(bytes))?;
        check_eq!(stored.payload.as_ptr(), clone.payload.as_ptr())?;
        check_eq!(ledger.snapshot()?.live_memory_bytes, bytes)?;
        check_eq!(ledger.snapshot()?.pending_escape_bytes, bytes)?;
        drop(guard);
        check_eq!(ledger.snapshot()?.interactive.active, 0)?;
        check_eq!(ledger.snapshot()?.reserved, ResourceNeed::default())?;
        check_eq!(ledger.snapshot()?.retained_memory_bytes, bytes)?;
        drop(stored);
        check_eq!(ledger.snapshot()?.retained_memory_bytes, bytes)?;
        drop(clone);
        let registration_bytes = quanta_index_core::index::RegisteredObject::allocation_bytes(0)?;
        check_eq!(ledger.snapshot()?.live_memory_bytes, registration_bytes)?;
        check_eq!(ledger.snapshot()?.retained_memory_bytes, registration_bytes)?;
        check_eq!(registration.reference, reference)?;
        check_eq!(registration.memory_bytes(), Some(registration_bytes))?;
        drop(registration);
        check_eq!(ledger.snapshot()?.live_memory_bytes, 0)?;
        check_eq!(ledger.snapshot()?.retained_memory_bytes, 0)?;
        Ok(())
    }

    #[test]
    fn retained_memory_real_object_denial_releases_wire_before_guard() -> Result<(), IndexError> {
        use quanta_index_core::index::ports::ImmutableObjectPort;
        let root = object_memory_root()?;
        let store = quanta_index_storage::FileObjectStore::open(root.path())?;
        let mut work = object_memory_budget()?;
        let reference = store
            .put(b"reserved wire then refused decoded bytes", &[], &mut work)?
            .reference;
        let ledger = NativeResourceLedger::new(plan())?;
        let guard = ReservationGuard::try_acquire(
            ledger.scheduler(NativeResourceClass::Control),
            plan().control,
        )
        .ok_or(IndexError::ResourceLimited)?;
        work.bind_memory_scope(guard.memory_scope());
        check!(matches!(
            store.get(reference, &mut work),
            Err(IndexError::ResourceLimited)
        ))?;
        let refused = ledger.snapshot()?;
        check_eq!(refused.live_memory_bytes, 0)?;
        check_eq!(refused.pending_escape_bytes, 0)?;
        check_eq!(refused.retained_memory_bytes, 0)?;
        drop(guard);
        check_eq!(ledger.snapshot()?.reserved, ResourceNeed::default())?;
        Ok(())
    }
    struct MapMemorySink;
    impl quanta_index_core::index::ports::ObjectWriteSink for MapMemorySink {
        fn reserve(
            &self,
            _object: &quanta_index_core::index::RegisteredObject,
        ) -> Result<(), IndexError> {
            Ok(())
        }
        fn installed(
            &self,
            _object: &quanta_index_core::index::RegisteredObject,
        ) -> Result<(), IndexError> {
            Ok(())
        }
        fn reserve_batch(
            &self,
            _objects: &[quanta_index_core::index::RegisteredObject],
        ) -> Result<(), IndexError> {
            Ok(())
        }
        fn installed_batch(
            &self,
            _objects: &[quanta_index_core::index::RegisteredObject],
        ) -> Result<(), IndexError> {
            Ok(())
        }
    }

    fn map_memory_fixture() -> Result<
        (
            tempfile::TempDir,
            quanta_index_storage::FileObjectStore,
            quanta_index_core::index::MapRoot,
            quanta_index_core::index::ObjectRef,
        ),
        IndexError,
    > {
        use quanta_index_core::index::{
            MapEdit, MapRoot,
            ports::{ImmutableObjectPort, PersistentMapPort},
        };
        let root = object_memory_root()?;
        let store = quanta_index_storage::FileObjectStore::open(root.path())?;
        let mut work = quanta_index_core::index::WorkBudget::new(
            quanta_index_core::index::WorkLimits {
                reads: 1000,
                writes: 1000,
                bytes: 1024 * 1024,
                mutations: 100,
            },
            std::time::Instant::now()
                .checked_add(std::time::Duration::from_secs(30))
                .ok_or(IndexError::ResourceLimited)?,
        );
        let value = store
            .put(b"fixed map lifetime golden", &[], &mut work)?
            .reference;
        let keys: [&[u8]; 6] = [b"", b"a", b"aa", b"ab", b"b", &[255]];
        let edits = keys
            .into_iter()
            .map(|key| MapEdit {
                key: key.to_vec(),
                expected: None,
                next: Some((value).into()),
            })
            .collect::<Vec<_>>();
        let map = store
            .apply(MapRoot(None), &edits, &MapMemorySink, &mut work)?
            .root;
        Ok((root, store, map, value))
    }

    fn map_memory_plan(stage: u64) -> Result<NativeResourcePlan, IndexError> {
        let mut configured = plan();
        configured.control.memory_bytes = stage;
        configured.interactive.memory_bytes = stage;
        configured.core.memory_bytes = stage;
        configured.enrichment.memory_bytes = stage;
        configured.capacity.memory_bytes =
            stage.checked_mul(5).ok_or(IndexError::ResourceLimited)?;
        Ok(configured)
    }

    #[test]
    fn retained_memory_real_map_page_and_cursor_keep_charge_after_execution()
    -> Result<(), IndexError> {
        use quanta_index_core::index::{MapPage, ports::PersistentMapPort};
        let (_root, store, map, value) = map_memory_fixture()?;
        let configured = map_memory_plan(64 * 1024)?;
        let ledger = NativeResourceLedger::new(configured)?;
        let guard = ReservationGuard::try_acquire(
            ledger.scheduler(NativeResourceClass::Interactive),
            configured.interactive,
        )
        .ok_or(IndexError::ResourceLimited)?;
        let mut work = object_memory_budget()?;
        work.bind_memory_scope(guard.memory_scope());
        let page = store.scan(map, None, 3, &mut work)?;
        let golden = [
            (Vec::new(), value),
            (b"a".to_vec(), value),
            (b"aa".to_vec(), value),
        ];
        let golden: Vec<(Vec<u8>, quanta_index_core::index::MapValue)> = golden
            .into_iter()
            .map(|(key, value)| (key, value.into()))
            .collect();
        check_eq!(page.entries.as_slice(), golden.as_slice())?;
        check!(page.has_more)?;
        let expected = MapPage::allocation_bytes(3)?
            .checked_add(3)
            .ok_or(IndexError::ResourceLimited)?;
        check_eq!(ledger.snapshot()?.live_memory_bytes, expected)?;
        let clone = page.clone();
        check_eq!(clone.entries.as_ptr(), page.entries.as_ptr())?;
        let mut cursor = page.cursor();
        let entry = cursor.next().ok_or(IndexError::ResourceLimited)?;
        check_eq!(entry.key()?, b"".as_slice())?;
        drop(guard);
        check_eq!(ledger.snapshot()?.reserved, ResourceNeed::default())?;
        check_eq!(ledger.snapshot()?.retained_memory_bytes, expected)?;
        drop(clone);
        drop(cursor);
        check_eq!(ledger.snapshot()?.retained_memory_bytes, expected)?;
        drop(entry);
        check_eq!(ledger.snapshot()?.live_memory_bytes, 0)?;
        check_eq!(ledger.snapshot()?.retained_memory_bytes, 0)?;
        Ok(())
    }

    #[test]
    fn retained_memory_real_map_scan_refusal_releases_all_partial_allocations()
    -> Result<(), IndexError> {
        use quanta_index_core::index::ports::PersistentMapPort;
        let (_root, store, map, _value) = map_memory_fixture()?;
        let configured = map_memory_plan(1024)?;
        let ledger = NativeResourceLedger::new(configured)?;
        let guard = ReservationGuard::try_acquire(
            ledger.scheduler(NativeResourceClass::Interactive),
            configured.interactive,
        )
        .ok_or(IndexError::ResourceLimited)?;
        let mut work = object_memory_budget()?;
        work.bind_memory_scope(guard.memory_scope());
        check!(matches!(
            store.scan(map, None, 3, &mut work),
            Err(IndexError::ResourceLimited)
        ))?;
        check_eq!(ledger.snapshot()?.live_memory_bytes, 0)?;
        check_eq!(ledger.snapshot()?.pending_escape_bytes, 0)?;
        drop(guard);
        check_eq!(ledger.snapshot()?.reserved, ResourceNeed::default())?;
        let next = ReservationGuard::try_acquire(
            ledger.scheduler(NativeResourceClass::Control),
            configured.control,
        )
        .ok_or(IndexError::ResourceLimited)?;
        drop(next);
        Ok(())
    }
    #[test]
    fn retained_memory_real_map_continuation_charges_both_pages_under_new_scope()
    -> Result<(), IndexError> {
        use quanta_index_core::index::{MapPage, ports::PersistentMapPort};
        let (_root, store, map, value) = map_memory_fixture()?;
        let configured = map_memory_plan(64 * 1024)?;
        let ledger = NativeResourceLedger::new(configured)?;
        let first_guard = ReservationGuard::try_acquire(
            ledger.scheduler(NativeResourceClass::Interactive),
            configured.interactive,
        )
        .ok_or(IndexError::ResourceLimited)?;
        let mut work = object_memory_budget()?;
        work.bind_memory_scope(first_guard.memory_scope());
        let first = store.scan(map, None, 3, &mut work)?;
        let continuation = first
            .clone()
            .cursor()
            .nth(2)
            .ok_or(IndexError::ResourceLimited)?;
        drop(first_guard);
        let first_bytes = MapPage::allocation_bytes(3)?
            .checked_add(3)
            .ok_or(IndexError::ResourceLimited)?;
        check_eq!(ledger.snapshot()?.retained_memory_bytes, first_bytes)?;
        let next_guard = ReservationGuard::try_acquire(
            ledger.scheduler(NativeResourceClass::Control),
            configured.control,
        )
        .ok_or(IndexError::ResourceLimited)?;
        work.bind_memory_scope(next_guard.memory_scope());
        let next = store.scan(map, Some(continuation.key()?), 3, &mut work)?;
        let golden = [
            (b"ab".to_vec(), value),
            (b"b".to_vec(), value),
            (vec![255], value),
        ];
        let golden: Vec<(Vec<u8>, quanta_index_core::index::MapValue)> = golden
            .into_iter()
            .map(|(key, value)| (key, value.into()))
            .collect();
        check_eq!(next.entries.as_slice(), golden.as_slice())?;
        check!(!next.has_more)?;
        let next_bytes = MapPage::allocation_bytes(3)?
            .checked_add(4)
            .ok_or(IndexError::ResourceLimited)?;
        let both = first_bytes
            .checked_add(next_bytes)
            .ok_or(IndexError::ResourceLimited)?;
        check_eq!(ledger.snapshot()?.live_memory_bytes, both)?;
        drop(next_guard);
        check_eq!(ledger.snapshot()?.reserved, ResourceNeed::default())?;
        check_eq!(ledger.snapshot()?.retained_memory_bytes, both)?;
        drop(first);
        check_eq!(ledger.snapshot()?.retained_memory_bytes, both)?;
        drop(continuation);
        check_eq!(ledger.snapshot()?.retained_memory_bytes, next_bytes)?;
        drop(next);
        check_eq!(ledger.snapshot()?.live_memory_bytes, 0)?;
        Ok(())
    }
}
