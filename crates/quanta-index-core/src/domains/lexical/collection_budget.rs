//! Shared work and retained-byte admission for one native lexical collection.
//!
//! Charges precede the operation they admit. Work is cumulative; memory is held
//! by a non-cloneable reservation until its allocation is no longer retained.
//! Cloning the budget shares all counters and its sticky first failure. There
//! are no mutexes, poisoned-state defaults, deadlines or cancellation flags here.

use std::sync::Arc;
use std::sync::atomic::{AtomicU8, AtomicU64, Ordering};

use quanta_index_contract::SearchPlaneErrorCodeV2;

use crate::error::CoreError;

const LIVE: u8 = 0;
const WORK_LIMIT: u8 = 1;
const WORK_OVERFLOW: u8 = 2;
const BYTE_LIMIT: u8 = 3;
const BYTE_OVERFLOW: u8 = 4;
const RELEASE_UNDERFLOW: u8 = 5;

#[derive(Debug)]
struct CollectionState {
    max_work: u64,
    max_bytes: u64,
    work: AtomicU64,
    resident: AtomicU64,
    peak: AtomicU64,
    failure: AtomicU8,
}

/// A single request's shared native collection resource ledger.
#[derive(Clone, Debug)]
pub struct LexicalCollectionBudget {
    state: Arc<CollectionState>,
}

impl LexicalCollectionBudget {
    pub fn new(max_work: u64, max_bytes: u64) -> Result<Self, CoreError> {
        if max_work == 0 || max_bytes == 0 {
            return Err(CoreError::InvalidContract(
                "lexical collection work and byte limits must both be positive".to_string(),
            ));
        }
        Ok(Self {
            state: Arc::new(CollectionState {
                max_work,
                max_bytes,
                work: AtomicU64::new(0),
                resident: AtomicU64::new(0),
                peak: AtomicU64::new(0),
                failure: AtomicU8::new(LIVE),
            }),
        })
    }

    /// Admit cumulative work before performing it. A race with another
    /// refusal may conservatively charge unused work, but can never authorize
    /// work beyond the limit or clear the first refusal.
    pub fn charge_work(&self, units: u64) -> Result<(), CoreError> {
        let _charged = self.add(
            &self.state.work,
            units,
            self.state.max_work,
            WORK_LIMIT,
            WORK_OVERFLOW,
        )?;
        self.check_live()
    }

    /// Reserve retained bytes before allocating. The caller must retain this
    /// guard at least as long as the admitted allocation (including transfers
    /// between segment and merge buffers).
    pub fn reserve_bytes(&self, bytes: u64) -> Result<LexicalMemoryReservation, CoreError> {
        let resident = self.add(
            &self.state.resident,
            bytes,
            self.state.max_bytes,
            BYTE_LIMIT,
            BYTE_OVERFLOW,
        )?;
        let _prior_peak = self.state.peak.fetch_max(resident, Ordering::AcqRel);
        let reservation = LexicalMemoryReservation {
            budget: self.clone(),
            bytes,
        };
        // An in-flight reservation that races the sticky refusal is released
        // on this error path before any caller can allocate with it.
        self.check_live()?;
        Ok(reservation)
    }

    /// First resource refusal, shared by all clones. Releasing bytes does not
    /// make a refused collection usable again.
    #[must_use]
    pub fn failure(&self) -> Option<CoreError> {
        let reason = self.state.failure.load(Ordering::Acquire);
        if reason == LIVE {
            return None;
        }
        let detail = match reason {
            WORK_LIMIT => "work limit exceeded",
            WORK_OVERFLOW => "work accounting overflow",
            BYTE_LIMIT => "retained-byte limit exceeded",
            BYTE_OVERFLOW => "retained-byte accounting overflow",
            RELEASE_UNDERFLOW => {
                return Some(CoreError::Storage(
                    "lexical collection reservation accounting underflow".to_string(),
                ));
            }
            _ => {
                return Some(CoreError::Storage(
                    "lexical collection has an invalid failure state".to_string(),
                ));
            }
        };
        Some(CoreError::Typed {
            code: SearchPlaneErrorCodeV2::LexicalCollectionBudgetExceeded,
            message: format!(
                "lexical collection: {detail}; max_work={}, max_bytes={}",
                self.state.max_work, self.state.max_bytes
            ),
        })
    }

    #[must_use]
    pub fn used_work(&self) -> u64 {
        self.state.work.load(Ordering::Acquire)
    }

    #[must_use]
    pub fn resident_bytes(&self) -> u64 {
        self.state.resident.load(Ordering::Acquire)
    }

    /// Peak logical byte reservations; this is not process RSS.
    #[must_use]
    pub fn peak_bytes(&self) -> u64 {
        self.state.peak.load(Ordering::Acquire)
    }

    fn check_live(&self) -> Result<(), CoreError> {
        match self.failure() {
            Some(error) => Err(error),
            None => Ok(()),
        }
    }

    fn fail(&self, reason: u8) -> CoreError {
        let _first =
            self.state
                .failure
                .compare_exchange(LIVE, reason, Ordering::AcqRel, Ordering::Acquire);
        // The state never returns to LIVE. Refuse internally too if that
        // invariant is ever broken; do not manufacture a successful admission.
        self.failure().unwrap_or_else(|| {
            CoreError::Storage("lexical collection lost its sticky refusal".to_string())
        })
    }

    fn add(
        &self,
        counter: &AtomicU64,
        amount: u64,
        maximum: u64,
        limit_reason: u8,
        overflow_reason: u8,
    ) -> Result<u64, CoreError> {
        self.check_live()?;
        match counter.fetch_update(Ordering::AcqRel, Ordering::Acquire, |current| {
            current.checked_add(amount).filter(|next| *next <= maximum)
        }) {
            Ok(prior) => prior
                .checked_add(amount)
                .ok_or_else(|| self.fail(overflow_reason)),
            Err(prior) => Err(self.fail(if prior.checked_add(amount).is_none() {
                overflow_reason
            } else {
                limit_reason
            })),
        }
    }

    fn release(&self, bytes: u64) {
        if self
            .state
            .resident
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |resident| {
                resident.checked_sub(bytes)
            })
            .is_err()
        {
            let _failure = self.fail(RELEASE_UNDERFLOW);
        }
    }
}

/// Non-cloneable ownership of one byte reservation. Moving the guard transfers
/// the reservation; dropping it releases exactly once, even after a refusal or
/// while unwinding. There is no public manual release or disarm operation.
#[must_use = "retain the reservation guard until the admitted allocation is released"]
#[derive(Debug)]
pub struct LexicalMemoryReservation {
    budget: LexicalCollectionBudget,
    bytes: u64,
}

impl Drop for LexicalMemoryReservation {
    fn drop(&mut self) {
        self.budget.release(self.bytes);
    }
}

#[cfg(test)]
mod tests {
    #![expect(
        clippy::panic_in_result_fn,
        reason = "Result-returning tests assert independent resource invariants"
    )]

    use std::sync::{Arc, Barrier};

    use super::{CoreError, LexicalCollectionBudget, LexicalMemoryReservation};

    type TestResult = Result<(), Box<dyn std::error::Error>>;

    fn assert_send_sync<T: Send + Sync>() {}

    #[test]
    fn collection_limits_are_positive_and_shared() -> TestResult {
        assert_send_sync::<LexicalCollectionBudget>();
        assert_send_sync::<LexicalMemoryReservation>();
        assert!(LexicalCollectionBudget::new(0, 1).is_err());
        assert!(LexicalCollectionBudget::new(1, 0).is_err());
        let budget = LexicalCollectionBudget::new(3, 5)?;
        budget.clone().charge_work(3)?;
        assert_eq!(budget.used_work(), 3);
        assert!(budget.charge_work(1).is_err());
        assert_eq!(budget.used_work(), 3);
        assert!(budget.charge_work(0).is_err());
        assert!(budget.reserve_bytes(0).is_err());
        Ok(())
    }

    #[test]
    fn reservations_account_peak_move_release_and_first_failure() -> TestResult {
        let budget = LexicalCollectionBudget::new(4, 8)?;
        let first = budget.reserve_bytes(3)?;
        let second = budget.clone().reserve_bytes(5)?;
        assert_eq!((budget.resident_bytes(), budget.peak_bytes()), (8, 8));
        drop(first);
        assert_eq!((budget.resident_bytes(), budget.peak_bytes()), (5, 8));
        let reused = budget.reserve_bytes(3)?;
        assert_eq!(budget.resident_bytes(), 8);
        drop(reused);
        let moved = second;
        assert!(budget.reserve_bytes(4).is_err());
        let first_failure = budget.failure().ok_or("missing byte refusal")?.to_string();
        drop(moved);
        assert_eq!((budget.resident_bytes(), budget.peak_bytes()), (0, 8));
        assert!(budget.charge_work(u64::MAX).is_err());
        assert!(budget.reserve_bytes(1).is_err());
        assert_eq!(budget.used_work(), 0);
        assert_eq!(
            budget.failure().ok_or("lost refusal")?.to_string(),
            first_failure
        );
        Ok(())
    }

    #[test]
    fn checked_counters_refuse_overflow_without_wrapping() -> TestResult {
        let work = LexicalCollectionBudget::new(u64::MAX, 1)?;
        work.charge_work(u64::MAX)?;
        assert!(work.charge_work(1).is_err());
        assert_eq!(work.used_work(), u64::MAX);
        let memory = LexicalCollectionBudget::new(1, u64::MAX)?;
        let held = memory.reserve_bytes(u64::MAX)?;
        assert!(memory.reserve_bytes(1).is_err());
        assert_eq!(memory.resident_bytes(), u64::MAX);
        drop(held);
        assert_eq!(memory.resident_bytes(), 0);
        Ok(())
    }

    #[test]
    fn concurrent_work_never_overadmits_or_loses_first_failure() -> TestResult {
        let budget = LexicalCollectionBudget::new(31, 1)?;
        let start = Arc::new(Barrier::new(9));
        let mut workers = Vec::new();
        for _ in 0..8 {
            let shared = budget.clone();
            let start = Arc::clone(&start);
            workers.push(std::thread::spawn(move || {
                let _ready = start.wait();
                let mut admitted = 0_u64;
                for _ in 0..32 {
                    if shared.charge_work(1).is_ok() {
                        admitted = admitted.saturating_add(1);
                    }
                }
                admitted
            }));
        }
        let _ready = start.wait();
        let mut admitted = 0_u64;
        for worker in workers {
            admitted = admitted.saturating_add(worker.join().map_err(|_| "worker panicked")?);
        }
        assert!(admitted <= 31);
        assert_eq!(budget.used_work(), 31);
        assert!(budget.failure().is_some());
        Ok(())
    }

    #[test]
    fn concurrent_reservations_retain_inflight_bytes_and_release_exactly_once() -> TestResult {
        let budget = LexicalCollectionBudget::new(1, 64)?;
        let barrier = Arc::new(Barrier::new(9));
        let mut workers = Vec::new();
        for _ in 0..8 {
            let shared = budget.clone();
            let barrier = Arc::clone(&barrier);
            workers.push(std::thread::spawn(move || {
                let held = shared.reserve_bytes(8);
                let _ready = barrier.wait();
                let _release = barrier.wait();
                held.map(drop)
            }));
        }
        let _ready = barrier.wait();
        let held_snapshot = (budget.resident_bytes(), budget.peak_bytes());
        let excess_refused = budget.reserve_bytes(1).is_err();
        let _release = barrier.wait();
        for worker in workers {
            worker.join().map_err(|_| "worker panicked")??;
        }
        assert_eq!(held_snapshot, (64, 64));
        assert!(excess_refused);
        assert_eq!((budget.resident_bytes(), budget.peak_bytes()), (0, 64));
        assert!(matches!(budget.failure(), Some(CoreError::Typed { .. })));
        Ok(())
    }
}
