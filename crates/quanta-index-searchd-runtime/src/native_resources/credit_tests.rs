use std::{
    sync::atomic::Ordering,
    thread,
    time::{Duration, Instant},
};

use quanta_index_core::index::{
    MemoryCreditLimits, MemoryLifetime, WorkBudget, WorkLimits,
    effort::{ReservationGuard, ResourceNeed},
};

use super::*;

fn plan(capacity_bytes: u64) -> NativeResourcePlan {
    let unit = ResourceNeed {
        cpu_units: 1,
        io_bytes: 1024,
        memory_bytes: 32 * 1024,
        provider_units: 1,
    };
    NativeResourcePlan {
        capacity: ResourceNeed {
            cpu_units: 4,
            io_bytes: 4096,
            memory_bytes: capacity_bytes,
            provider_units: 4,
        },
        control: unit,
        interactive: unit,
        core: unit,
        enrichment: unit,
    }
}

fn budget() -> Result<WorkBudget, IndexError> {
    Ok(WorkBudget::new(
        WorkLimits {
            reads: 100,
            writes: 100,
            bytes: 100_000,
            mutations: 100,
        },
        Instant::now()
            .checked_add(Duration::from_secs(30))
            .ok_or(IndexError::ResourceLimited)?,
    ))
}

fn limits() -> MemoryCreditLimits {
    MemoryCreditLimits {
        temporary_data_bytes: 128,
        retained_data_bytes: 256,
        max_live_temporary_leases: 1,
        max_live_retained_leases: 2,
    }
}

#[test]
fn scoped_credit_reuses_released_child_and_does_not_pin_peak_after_close() -> Result<(), IndexError>
{
    let configured = plan(256 * 1024);
    let ledger = NativeResourceLedger::new(configured)?;
    let guard =
        ReservationGuard::try_acquire(ledger.scheduler(NativeResourceClass::Core), configured.core)
            .ok_or(IndexError::ResourceLimited)?;
    let mut work = budget()?;
    work.bind_memory_scope(guard.memory_scope());
    let escaped = {
        let credit = work
            .try_scoped_memory_credit(limits())?
            .ok_or(IndexError::ResourceLimited)?;
        let first = credit
            .reserve_memory(100, MemoryLifetime::Temporary)?
            .ok_or(IndexError::ResourceLimited)?;
        let live = ledger.snapshot()?.live_memory_bytes;
        drop(first);
        let second = credit
            .reserve_memory(110, MemoryLifetime::Temporary)?
            .ok_or(IndexError::ResourceLimited)?;
        if ledger.snapshot()?.live_memory_bytes <= live.saturating_sub(100) {
            return Err(invalid_ledger());
        }
        drop(second);
        let retained = credit
            .reserve_memory(120, MemoryLifetime::Retained)?
            .ok_or(IndexError::ResourceLimited)?;
        let clone = retained.clone();
        drop(retained);
        clone
    };
    // The large unused data and backing credit is returned at scope close.
    let open = ledger.snapshot()?;
    if open.pending_escape_bytes <= 120 || open.live_memory_bytes >= 32 * 1024 {
        return Err(invalid_ledger());
    }
    let ordinary = work
        .reserve_memory(64, MemoryLifetime::Temporary)?
        .ok_or(IndexError::ResourceLimited)?;
    drop(ordinary);
    drop(guard);
    let detached = ledger.snapshot()?;
    if detached.reserved != ResourceNeed::default()
        || detached.pending_escape_bytes != 0
        || detached.retained_memory_bytes != detached.live_memory_bytes
    {
        return Err(invalid_ledger());
    }
    drop(escaped);
    let settled = ledger.snapshot()?;
    if settled.live_memory_bytes != 0 || settled.retained_memory_bytes != 0 {
        return Err(invalid_ledger());
    }
    Ok(())
}

#[test]
fn optional_credit_refuses_insufficient_future_floor_without_terminal_denial()
-> Result<(), IndexError> {
    let configured = plan(128 * 1024);
    let ledger = NativeResourceLedger::new(configured)?;
    let guard =
        ReservationGuard::try_acquire(ledger.scheduler(NativeResourceClass::Core), configured.core)
            .ok_or(IndexError::ResourceLimited)?;
    let mut work = budget()?;
    work.bind_memory_scope(guard.memory_scope());
    if work.try_scoped_memory_credit(limits())?.is_some() {
        return Err(invalid_ledger());
    }
    // The original mandatory path remains available under the same budget.
    let original = work
        .reserve_memory(64, MemoryLifetime::Temporary)?
        .ok_or(IndexError::ResourceLimited)?;
    drop(original);
    if ledger.snapshot()?.live_memory_bytes != 0 || ledger.snapshot()?.pending_escape_bytes != 0 {
        return Err(invalid_ledger());
    }
    drop(guard);
    Ok(())
}

#[test]
fn optional_allocation_inside_credit_uses_parent_without_starving_mandatory_child()
-> Result<(), IndexError> {
    let configured = plan(256 * 1024);
    let ledger = NativeResourceLedger::new(configured)?;
    let guard =
        ReservationGuard::try_acquire(ledger.scheduler(NativeResourceClass::Core), configured.core)
            .ok_or(IndexError::ResourceLimited)?;
    let mut work = budget()?;
    work.bind_memory_scope(guard.memory_scope());
    let limits = MemoryCreditLimits {
        temporary_data_bytes: 31_000,
        retained_data_bytes: 0,
        max_live_temporary_leases: 1,
        max_live_retained_leases: 0,
    };
    let scoped = work
        .try_scoped_memory_credit(limits)?
        .ok_or(IndexError::ResourceLimited)?;
    let optional = scoped
        .try_reserve_memory_for_optimization(1, MemoryLifetime::Temporary)?
        .ok_or(IndexError::ResourceLimited)?;
    if scoped
        .try_reserve_memory_for_optimization(2_000, MemoryLifetime::Temporary)?
        .is_some()
        || scoped.first_denial().is_some()
    {
        return Err(invalid_ledger());
    }
    // Keep the optional parent allocation live: the sole child slot and its
    // entire data credit must still be available to the mandatory operation.
    let required = scoped
        .reserve_memory(31_000, MemoryLifetime::Temporary)?
        .ok_or(IndexError::ResourceLimited)?;
    drop((required, optional, scoped, guard));
    if ledger.snapshot()?.live_memory_bytes != 0 || ledger.snapshot()?.pending_escape_bytes != 0 {
        return Err(invalid_ledger());
    }
    Ok(())
}

#[test]
fn stale_scope_and_execution_close_refuse_new_children_but_release_escaped_child()
-> Result<(), IndexError> {
    let configured = plan(256 * 1024);
    let ledger = NativeResourceLedger::new(configured)?;
    let guard =
        ReservationGuard::try_acquire(ledger.scheduler(NativeResourceClass::Core), configured.core)
            .ok_or(IndexError::ResourceLimited)?;
    let parent = guard.memory_scope().ok_or_else(invalid_ledger)?;
    let credit = parent
        .try_reserve_credit(limits())?
        .ok_or(IndexError::ResourceLimited)?;
    let child_scope = credit.scope();
    let escaped = child_scope.reserve(120, MemoryLifetime::Retained)?;
    drop(guard);
    if !matches!(
        child_scope.reserve(1, MemoryLifetime::Retained),
        Err(IndexError::BackendViolation(_))
    ) {
        return Err(invalid_ledger());
    }
    drop(credit);
    drop(child_scope);
    drop(escaped);
    if ledger.snapshot()?.live_memory_bytes != 0 {
        return Err(invalid_ledger());
    }
    Ok(())
}

#[test]
fn closed_credit_cannot_spend_a_later_credit_on_the_same_execution() -> Result<(), IndexError> {
    let configured = plan(256 * 1024);
    let ledger = NativeResourceLedger::new(configured)?;
    let guard =
        ReservationGuard::try_acquire(ledger.scheduler(NativeResourceClass::Core), configured.core)
            .ok_or(IndexError::ResourceLimited)?;
    let parent = guard.memory_scope().ok_or_else(invalid_ledger)?;
    let first = parent
        .try_reserve_credit(limits())?
        .ok_or(IndexError::ResourceLimited)?;
    let stale = first.scope();
    drop(first);
    let second = parent
        .try_reserve_credit(limits())?
        .ok_or(IndexError::ResourceLimited)?;
    if !matches!(
        stale.reserve(1, MemoryLifetime::Temporary),
        Err(IndexError::BackendViolation(_))
    ) {
        return Err(invalid_ledger());
    }
    let actual = second.scope().reserve(1, MemoryLifetime::Temporary)?;
    drop(actual);
    drop((second, stale, guard));
    if ledger.snapshot()?.live_memory_bytes != 0 {
        return Err(invalid_ledger());
    }
    Ok(())
}

#[test]
fn temporary_child_outlives_closed_credit_without_touching_later_credit() -> Result<(), IndexError>
{
    let configured = plan(256 * 1024);
    let ledger = NativeResourceLedger::new(configured)?;
    let guard =
        ReservationGuard::try_acquire(ledger.scheduler(NativeResourceClass::Core), configured.core)
            .ok_or(IndexError::ResourceLimited)?;
    let parent = guard.memory_scope().ok_or_else(invalid_ledger)?;
    let first = parent
        .try_reserve_credit(MemoryCreditLimits {
            temporary_data_bytes: 128,
            retained_data_bytes: 0,
            max_live_temporary_leases: 1,
            max_live_retained_leases: 0,
        })?
        .ok_or(IndexError::ResourceLimited)?;
    let stale = first.scope();
    let escaped = stale.reserve(96, MemoryLifetime::Temporary)?;
    drop(first);
    let second = parent
        .try_reserve_credit(limits())?
        .ok_or(IndexError::ResourceLimited)?;
    let current = second.scope().reserve(100, MemoryLifetime::Temporary)?;
    let before_release = ledger.snapshot()?.live_memory_bytes;
    drop(escaped);
    if ledger.snapshot()?.live_memory_bytes >= before_release {
        return Err(invalid_ledger());
    }
    if !matches!(
        second.scope().reserve(1, MemoryLifetime::Temporary),
        Err(IndexError::ResourceLimited)
    ) {
        return Err(invalid_ledger());
    }
    if !matches!(
        stale.reserve(1, MemoryLifetime::Temporary),
        Err(IndexError::BackendViolation(_))
    ) {
        return Err(invalid_ledger());
    }
    drop((current, second, stale, guard));
    let settled = ledger.snapshot()?;
    if settled.live_memory_bytes != 0 || settled.pending_escape_bytes != 0 {
        return Err(invalid_ledger());
    }
    Ok(())
}

#[test]
fn temporary_child_escaping_execution_fails_closed_until_its_last_drop() -> Result<(), IndexError> {
    let configured = plan(256 * 1024);
    let ledger = NativeResourceLedger::new(configured)?;
    let guard =
        ReservationGuard::try_acquire(ledger.scheduler(NativeResourceClass::Core), configured.core)
            .ok_or(IndexError::ResourceLimited)?;
    let parent = guard.memory_scope().ok_or_else(invalid_ledger)?;
    let credit = parent
        .try_reserve_credit(limits())?
        .ok_or(IndexError::ResourceLimited)?;
    let escaped = credit.scope().reserve(100, MemoryLifetime::Temporary)?;
    drop(guard);
    if ledger.snapshot().is_ok() {
        return Err(invalid_ledger());
    }
    drop((escaped, credit));
    if !ledger.invalid.load(Ordering::Acquire) {
        return Err(invalid_ledger());
    }
    Ok(())
}

#[test]
fn child_release_racing_execution_close_settles_once() -> Result<(), IndexError> {
    let configured = plan(256 * 1024);
    let ledger = NativeResourceLedger::new(configured)?;
    let guard =
        ReservationGuard::try_acquire(ledger.scheduler(NativeResourceClass::Core), configured.core)
            .ok_or(IndexError::ResourceLimited)?;
    let parent = guard.memory_scope().ok_or_else(invalid_ledger)?;
    let credit = parent
        .try_reserve_credit(limits())?
        .ok_or(IndexError::ResourceLimited)?;
    let child = credit.scope().reserve(120, MemoryLifetime::Retained)?;
    let worker = thread::spawn(move || drop(child));
    drop(guard);
    worker.join().map_err(|_panic| invalid_ledger())?;
    drop(credit);
    if ledger.snapshot()?.live_memory_bytes != 0
        || ledger.snapshot()?.pending_escape_bytes != 0
        || ledger.snapshot()?.reserved != ResourceNeed::default()
    {
        return Err(invalid_ledger());
    }
    Ok(())
}
