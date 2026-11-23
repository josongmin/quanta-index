//! Physical SQLite FD custody for a future product-owned Unix VFS boundary.
//!
//! A VFS adapter must hold this ledger's mutex across each real `open` and
//! `close` syscall. The ledger cannot make unrelated process code obey that
//! rule; production registration requires exclusive SQLite syscall custody.

#![forbid(unsafe_code)]

use std::{
    num::{NonZeroU64, NonZeroUsize},
    os::fd::RawFd,
    panic::{AssertUnwindSafe, catch_unwind},
    sync::{Mutex, MutexGuard},
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct FileIdentity {
    pub device: u64,
    pub inode: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct FdToken {
    fd: RawFd,
    generation: NonZeroU64,
}

impl FdToken {
    pub fn fd(self) -> RawFd {
        self.fd
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RetirementReceipt {
    pub root: NonZeroU64,
    pub charge: NonZeroU64,
    pub identity: FileIdentity,
    pub generation: NonZeroU64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum OpenDisposition {
    Ready(FdToken),
    /// Open succeeded, but its inode or FD custody was not proven. Never use
    /// this FD for SQLite I/O or refund its charge from this ledger.
    Quarantined(FdToken),
}

#[derive(Debug, Eq, PartialEq)]
pub enum LedgerError {
    Poisoned,
    InvalidFd,
    GenerationExhausted,
    MemoryUnavailable,
    Full,
    UnknownToken,
    NotCurrent,
}

#[derive(Debug)]
pub enum OpenError<E> {
    Syscall(E),
    Ledger(LedgerError),
}

#[derive(Debug)]
pub enum CloseReport<E> {
    Closed(Option<RetirementReceipt>),
    /// The owned syscall was attempted exactly once. EINTR, EBADF, and other
    /// errors all leave the original charge quarantined.
    Uncertain(E),
}

#[derive(Clone, Copy)]
enum Physical {
    Live,
    Retired,
    Uncertain,
}

struct Entry {
    generation: NonZeroU64,
    fd: RawFd,
    root: NonZeroU64,
    charge: NonZeroU64,
    identity: Option<FileIdentity>,
    physical: Physical,
    owner_released: bool,
    quarantined: bool,
}

struct State {
    next_generation: u64,
    // Fully allocated before the first syscall. Quarantined entries keep
    // their slot until process-level reconciliation, never grow the vector.
    entries: Vec<Option<Entry>>,
}

pub struct FdLedger {
    state: Mutex<State>,
}

impl FdLedger {
    /// Minimum requested slot bytes. `try_reserve_exact` may allocate more,
    /// so this is not a hard-memory-quota admission amount.
    pub fn minimum_slot_backing_bytes(max_entries: NonZeroUsize) -> Result<u64, LedgerError> {
        let bytes = std::mem::size_of::<Option<Entry>>()
            .checked_mul(max_entries.get())
            .ok_or(LedgerError::MemoryUnavailable)?;
        u64::try_from(bytes).map_err(|_| LedgerError::MemoryUnavailable)
    }

    pub fn with_capacity(max_entries: NonZeroUsize) -> Result<Self, LedgerError> {
        let _minimum_size = Self::minimum_slot_backing_bytes(max_entries)?;
        let mut entries = Vec::new();
        entries
            .try_reserve_exact(max_entries.get())
            .map_err(|_| LedgerError::MemoryUnavailable)?;
        entries.resize_with(max_entries.get(), || None);
        Ok(Self {
            state: Mutex::new(State {
                next_generation: 0,
                entries,
            }),
        })
    }

    fn lock(&self) -> Result<MutexGuard<'_, State>, LedgerError> {
        self.state.lock().map_err(|_| LedgerError::Poisoned)
    }

    /// Run the actual `openat` and `fstat` while holding the custody lock.
    /// `witness` must not close the FD. A failed witness returns `None` so the
    /// successfully opened FD is still registered and its charge retained.
    /// The caller must reserve `charge` first and retain it on quarantine.
    pub fn open_with<E>(
        &self,
        root: NonZeroU64,
        charge: NonZeroU64,
        open_fd: impl FnOnce() -> Result<RawFd, E>,
        witness: impl FnOnce(RawFd) -> Option<FileIdentity>,
    ) -> Result<OpenDisposition, OpenError<E>> {
        let mut state = self.lock().map_err(OpenError::Ledger)?;
        let generation = NonZeroU64::new(
            state
                .next_generation
                .checked_add(1)
                .ok_or(OpenError::Ledger(LedgerError::GenerationExhausted))?,
        )
        .ok_or(OpenError::Ledger(LedgerError::GenerationExhausted))?;
        let slot = state
            .entries
            .iter()
            .position(Option::is_none)
            .ok_or(OpenError::Ledger(LedgerError::Full))?;
        // A panic after the OS returned an FD but before the closure returned
        // cannot be reconstructed. Terminate instead of continuing without
        // custody. The FFI adapter must never unwind across SQLite C frames.
        let fd = match catch_unwind(AssertUnwindSafe(open_fd)) {
            Ok(result) => result.map_err(OpenError::Syscall)?,
            Err(_) => std::process::abort(),
        };
        if fd < 0 {
            return Err(OpenError::Ledger(LedgerError::InvalidFd));
        }
        let identity = match catch_unwind(AssertUnwindSafe(|| witness(fd))) {
            Ok(identity) => identity,
            Err(_) => std::process::abort(),
        };
        state.next_generation = generation.get();
        // Two simultaneously live records for one numeric FD mean some code
        // bypassed custody. Neither record can be safely retired: stop now.
        if state
            .entries
            .iter()
            .flatten()
            .any(|entry| entry.fd == fd && matches!(entry.physical, Physical::Live))
        {
            std::process::abort();
        }
        let quarantined = identity.is_none();
        state.entries[slot] = Some(Entry {
            generation,
            fd,
            root,
            charge,
            identity,
            physical: Physical::Live,
            owner_released: false,
            quarantined,
        });
        let token = FdToken { fd, generation };
        Ok(if quarantined {
            OpenDisposition::Quarantined(token)
        } else {
            OpenDisposition::Ready(token)
        })
    }

    pub fn current(&self, fd: RawFd) -> Result<Option<FdToken>, LedgerError> {
        let state = self.lock()?;
        Ok(state
            .entries
            .iter()
            .flatten()
            .filter(|entry| entry.fd == fd && !matches!(entry.physical, Physical::Retired))
            .max_by_key(|entry| entry.generation)
            .map(|entry| FdToken {
                fd,
                generation: entry.generation,
            }))
    }

    /// This is the only path that issues a refund receipt. A logical xClose
    /// may precede physical close by an arbitrary number of other operations.
    pub fn release_owner(&self, token: FdToken) -> Result<Option<RetirementReceipt>, LedgerError> {
        let mut state = self.lock()?;
        let entry =
            find_entry_mut(&mut state, token.generation).ok_or(LedgerError::UnknownToken)?;
        if entry.fd != token.fd || entry.owner_released {
            return Err(LedgerError::UnknownToken);
        }
        entry.owner_released = true;
        Ok(retire_if_proven(&mut state, token.generation))
    }

    /// Invoke the real close exactly once while holding the custody lock.
    /// Never retry a numeric FD after an error, including EINTR or EBADF.
    pub fn close_with<E>(
        &self,
        token: FdToken,
        close_once: impl FnOnce(RawFd) -> Result<(), E>,
    ) -> Result<CloseReport<E>, LedgerError> {
        let mut state = self.lock()?;
        if current_token(&state, token.fd) != Some(token)
            || !state.entries.iter().flatten().any(|entry| {
                entry.generation == token.generation && matches!(entry.physical, Physical::Live)
            })
        {
            return Err(LedgerError::NotCurrent);
        }
        let close_result = match catch_unwind(AssertUnwindSafe(|| close_once(token.fd))) {
            Ok(result) => result,
            Err(_) => std::process::abort(),
        };
        match close_result {
            Ok(()) => {
                let entry = find_entry_mut(&mut state, token.generation)
                    .ok_or(LedgerError::UnknownToken)?;
                entry.physical = Physical::Retired;
                Ok(CloseReport::Closed(retire_if_proven(
                    &mut state,
                    token.generation,
                )))
            }
            Err(error) => {
                let entry = find_entry_mut(&mut state, token.generation)
                    .ok_or(LedgerError::UnknownToken)?;
                entry.physical = Physical::Uncertain;
                Ok(CloseReport::Uncertain(error))
            }
        }
    }
}

fn retire_if_proven(state: &mut State, generation: NonZeroU64) -> Option<RetirementReceipt> {
    let slot = state.entries.iter().position(|entry| {
        entry
            .as_ref()
            .is_some_and(|entry| entry.generation == generation)
    })?;
    let entry = state.entries[slot].as_ref()?;
    if !entry.owner_released || !matches!(entry.physical, Physical::Retired) || entry.quarantined {
        return None;
    }
    let identity = entry.identity?;
    let receipt = RetirementReceipt {
        root: entry.root,
        charge: entry.charge,
        identity,
        generation,
    };
    state.entries[slot] = None;
    Some(receipt)
}

fn find_entry_mut(state: &mut State, generation: NonZeroU64) -> Option<&mut Entry> {
    state
        .entries
        .iter_mut()
        .flatten()
        .find(|entry| entry.generation == generation)
}

fn current_token(state: &State, fd: RawFd) -> Option<FdToken> {
    state
        .entries
        .iter()
        .flatten()
        .filter(|entry| entry.fd == fd && !matches!(entry.physical, Physical::Retired))
        .max_by_key(|entry| entry.generation)
        .map(|entry| FdToken {
            fd,
            generation: entry.generation,
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn id(value: u64) -> NonZeroU64 {
        NonZeroU64::new(value).unwrap()
    }

    fn identity(inode: u64) -> FileIdentity {
        FileIdentity { device: 7, inode }
    }

    #[test]
    fn delayed_close_releases_only_after_real_retirement() {
        let ledger = FdLedger::with_capacity(NonZeroUsize::new(16).unwrap()).unwrap();
        let OpenDisposition::Ready(token) = ledger
            .open_with(id(1), id(11), || Ok::<_, ()>(5), |_| Some(identity(19)))
            .unwrap()
        else {
            panic!("expected witnessed open");
        };
        assert_eq!(ledger.release_owner(token), Ok(None));
        let CloseReport::Closed(Some(receipt)) =
            ledger.close_with(token, |_| Ok::<_, ()>(())).unwrap()
        else {
            panic!("expected physical retirement");
        };
        assert_eq!((receipt.charge, receipt.identity), (id(11), identity(19)));
        assert_eq!(ledger.current(5), Ok(None));
        assert_eq!(ledger.release_owner(token), Err(LedgerError::UnknownToken));
    }

    #[test]
    fn uncertain_close_keeps_old_charge_across_fd_reuse() {
        let ledger = FdLedger::with_capacity(NonZeroUsize::new(16).unwrap()).unwrap();
        let OpenDisposition::Ready(old) = ledger
            .open_with(id(1), id(11), || Ok::<_, ()>(5), |_| Some(identity(19)))
            .unwrap()
        else {
            panic!("expected witnessed open");
        };
        assert_eq!(ledger.release_owner(old), Ok(None));
        let mut attempts = 0;
        assert!(matches!(
            ledger.close_with(old, |_| {
                attempts += 1;
                Err::<(), _>("EINTR")
            }),
            Ok(CloseReport::Uncertain("EINTR"))
        ));
        assert_eq!(attempts, 1);
        assert!(matches!(
            ledger.close_with(old, |_| {
                attempts += 1;
                Ok::<_, &str>(())
            }),
            Err(LedgerError::NotCurrent)
        ));
        assert_eq!(attempts, 1);
        let OpenDisposition::Ready(new) = ledger
            .open_with(id(2), id(12), || Ok::<_, ()>(5), |_| Some(identity(20)))
            .unwrap()
        else {
            panic!("FD reuse must be a new generation");
        };
        assert_ne!(old.generation, new.generation);
        assert_eq!(ledger.current(5), Ok(Some(new)));
        assert!(matches!(
            ledger.close_with(old, |_| Ok::<_, ()>(())),
            Err(LedgerError::NotCurrent)
        ));
        assert_eq!(ledger.release_owner(new), Ok(None));
        assert!(
            matches!(ledger.close_with(new, |_| Ok::<_, ()>(())), Ok(CloseReport::Closed(Some(receipt))) if receipt.charge == id(12))
        );
        // The old charge deliberately has no retirement receipt.
        assert_eq!(ledger.release_owner(old), Err(LedgerError::UnknownToken));
    }

    #[test]
    fn missing_witness_never_refunds() {
        let ledger = FdLedger::with_capacity(NonZeroUsize::new(16).unwrap()).unwrap();
        let OpenDisposition::Quarantined(token) = ledger
            .open_with(id(1), id(11), || Ok::<_, ()>(5), |_| None)
            .unwrap()
        else {
            panic!("missing inode witness must quarantine");
        };
        assert_eq!(ledger.release_owner(token), Ok(None));
        assert!(matches!(
            ledger.close_with(token, |_| Ok::<_, ()>(())),
            Ok(CloseReport::Closed(None))
        ));
    }

    #[test]
    fn ebadf_is_not_retried_or_refunded() {
        let ledger = FdLedger::with_capacity(NonZeroUsize::new(16).unwrap()).unwrap();
        let OpenDisposition::Ready(token) = ledger
            .open_with(id(1), id(11), || Ok::<_, ()>(5), |_| Some(identity(19)))
            .unwrap()
        else {
            panic!("expected witnessed open");
        };
        let mut calls = 0;
        assert!(matches!(
            ledger.close_with(token, |_| {
                calls += 1;
                Err::<(), _>("EBADF")
            }),
            Ok(CloseReport::Uncertain("EBADF"))
        ));
        assert_eq!(calls, 1);
        assert!(matches!(
            ledger.close_with(token, |_| {
                calls += 1;
                Ok::<_, &str>(())
            }),
            Err(LedgerError::NotCurrent)
        ));
        assert_eq!(calls, 1);
        assert_eq!(ledger.release_owner(token), Ok(None));
        assert_eq!(ledger.current(5), Ok(Some(token)));
    }

    #[test]
    fn physical_close_before_owner_release_refunds_once() {
        let ledger = FdLedger::with_capacity(NonZeroUsize::new(16).unwrap()).unwrap();
        let OpenDisposition::Ready(token) = ledger
            .open_with(id(1), id(11), || Ok::<_, ()>(5), |_| Some(identity(19)))
            .unwrap()
        else {
            panic!("expected witnessed open");
        };
        assert!(matches!(
            ledger.close_with(token, |_| Ok::<_, ()>(())),
            Ok(CloseReport::Closed(None))
        ));
        let receipt = ledger.release_owner(token).unwrap().unwrap();
        assert_eq!(receipt.charge, id(11));
        assert_eq!(ledger.release_owner(token), Err(LedgerError::UnknownToken));
    }

    #[test]
    fn full_registry_refuses_before_open_syscall() {
        let ledger = FdLedger::with_capacity(NonZeroUsize::new(1).unwrap()).unwrap();
        let OpenDisposition::Ready(_held) = ledger
            .open_with(id(1), id(11), || Ok::<_, ()>(5), |_| Some(identity(19)))
            .unwrap()
        else {
            panic!("expected witnessed open");
        };
        let mut called = false;
        let result = ledger.open_with(
            id(1),
            id(12),
            || {
                called = true;
                Ok::<_, ()>(6)
            },
            |_| Some(identity(20)),
        );
        assert!(matches!(result, Err(OpenError::Ledger(LedgerError::Full))));
        assert!(!called);
    }
}
