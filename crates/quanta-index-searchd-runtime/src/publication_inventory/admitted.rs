//! Admission/retirement facets for the ONE canonical boot inventory walk.
use super::*;
use quanta_index_core::index::{
    IndexError, MemoryLease, MemoryLifetime, WorkBudget, WorkUsage,
    existing_file::ExistingFileReadFailureV1,
};
use std::{cell::Cell, rc::Rc};

pub(crate) struct InventoryMemory {
    grants: Vec<MemoryLease>,
}
impl InventoryMemory {
    fn new() -> Self {
        Self { grants: Vec::new() }
    }
    fn reserve(
        &mut self,
        bytes: u64,
        budget: &WorkBudget,
        lifetime: MemoryLifetime,
    ) -> Result<(), ExistingFileReadFailureV1> {
        if bytes == 0 {
            return Ok(());
        }
        let growth = if self.grants.len() == self.grants.capacity() {
            self.grants
                .capacity()
                .checked_mul(2)
                .ok_or(IndexError::ResourceLimited)?
                .max(1)
        } else {
            0
        };
        let backing = u64::try_from(
            std::alloc::Layout::array::<MemoryLease>(growth)
                .map_err(|_| IndexError::ResourceLimited)?
                .size(),
        )
        .map_err(|_| IndexError::ResourceLimited)?;
        let grant = budget
            .reserve_memory(
                bytes
                    .checked_add(backing)
                    .ok_or(IndexError::ResourceLimited)?,
                lifetime,
            )?
            .ok_or(IndexError::ResourceLimited)?;
        if growth > 0 {
            self.grants
                .try_reserve_exact(growth.saturating_sub(self.grants.len()))
                .map_err(|_| IndexError::ResourceLimited)?;
        }
        self.grants.push(grant);
        Ok(())
    }
}
pub(super) struct FdState {
    active: Cell<usize>,
    close_uncertain: Cell<bool>,
    max: usize,
    recovery_fault: Option<crate::publication_file_capacity::RecoveryReadCloseFaultV1>,
}
impl FdState {
    fn mark_close_uncertain(&self) {
        self.close_uncertain.set(true);
        if let Some(fault) = &self.recovery_fault {
            fault.mark();
        }
    }

    fn require_healthy(&self) -> Result<(), CoreError> {
        if self.close_uncertain.get() {
            return Err(inventory_error(
                "inventory descriptor close was not witnessed",
            ));
        }
        Ok(())
    }
    fn claim(&self) -> Result<(), CoreError> {
        self.require_healthy()?;
        let next = self
            .active
            .get()
            .checked_add(1)
            .ok_or_else(|| inventory_capacity("inventory descriptor count overflow"))?;
        if next > self.max {
            return Err(inventory_capacity("inventory descriptor ceiling exceeded"));
        }
        self.active.set(next);
        Ok(())
    }
    fn release(&self) {
        match self.active.get().checked_sub(1) {
            Some(n) => self.active.set(n),
            None => self.mark_close_uncertain(),
        }
    }
    fn require_retired(&self) -> Result<(), CoreError> {
        if self.active.get() != 0 || self.close_uncertain.get() {
            self.mark_close_uncertain();
            Err(inventory_error(
                "inventory descriptor retirement was not witnessed",
            ))
        } else {
            Ok(())
        }
    }
}
pub(super) trait InventoryControl {
    type Error: From<CoreError>;
    type Memory;
    const ADMITTED: bool;
    fn checkpoint(&mut self, label: &'static str) -> Result<(), Self::Error>;
    fn charge(&mut self, reads: u64, bytes: u64) -> Result<(), Self::Error>;
    fn retain(&mut self, bytes: u64) -> Result<(), Self::Error>;
    fn scratch(&mut self, bytes: u64) -> Result<(), Self::Error>;
    fn temporary(&mut self, bytes: u64) -> Result<Self::Memory, Self::Error>;
    fn fd_state(&self) -> Rc<FdState>;
}
pub(super) struct ScanFile {
    file: Option<File>,
    state: Rc<FdState>,
}
impl ScanFile {
    pub(super) fn open<C: InventoryControl>(
        control: &mut C,
        open: impl FnOnce() -> Result<File, CoreError>,
    ) -> Result<Self, C::Error> {
        control.charge(1, 0)?;
        let state = control.fd_state();
        state.claim()?;
        let file = match open() {
            Ok(file) => file,
            Err(error) => {
                state.release();
                return Err(error.into());
            }
        };
        Ok(Self {
            file: Some(file),
            state,
        })
    }
    pub(super) fn file(&self) -> Result<&File, CoreError> {
        self.file
            .as_ref()
            .ok_or_else(|| inventory_error("inventory directory retired"))
    }
}
impl Drop for ScanFile {
    fn drop(&mut self) {
        if let Some(file) = self.file.take() {
            if rusqlite::ffi::quanta_capacity_vfs::close_owned_file(file).is_err() {
                self.state.mark_close_uncertain();
            } else {
                self.state.release();
            }
        }
    }
}
pub(super) struct Ordinary<'a> {
    request: &'a RequestBudgetV1,
    state: Rc<FdState>,
}
impl<'a> Ordinary<'a> {
    pub(super) fn new(request: &'a RequestBudgetV1) -> Self {
        Self {
            request,
            state: Rc::new(FdState {
                active: Cell::new(0),
                close_uncertain: Cell::new(false),
                max: usize::MAX,
                recovery_fault: None,
            }),
        }
    }
}
impl InventoryControl for Ordinary<'_> {
    type Error = CoreError;
    type Memory = ();
    const ADMITTED: bool = false;
    fn checkpoint(&mut self, label: &'static str) -> Result<(), CoreError> {
        self.request.checkpoint(label)
    }
    fn charge(&mut self, _: u64, _: u64) -> Result<(), CoreError> {
        Ok(())
    }
    fn retain(&mut self, _: u64) -> Result<(), CoreError> {
        Ok(())
    }
    fn scratch(&mut self, _: u64) -> Result<(), CoreError> {
        Ok(())
    }
    fn temporary(&mut self, _: u64) -> Result<(), CoreError> {
        Ok(())
    }
    fn fd_state(&self) -> Rc<FdState> {
        Rc::clone(&self.state)
    }
}
pub(super) struct Admitted<'a> {
    budget: &'a mut WorkBudget,
    pub(super) memory: InventoryMemory,
    state: Rc<FdState>,
    scratch: InventoryMemory,
}
impl<'a> Admitted<'a> {
    pub(super) fn new(
        budget: &'a mut WorkBudget,
        max_handles: usize,
        registration: &crate::publication_file_capacity::RecoveryReadRootRegistrationV1,
    ) -> Result<Self, ExistingFileReadFailureV1> {
        if max_handles == 0 {
            return Err(IndexError::ResourceLimited.into());
        }
        budget.checkpoint()?;
        let memory = InventoryMemory::new();
        let mut scratch = InventoryMemory::new();
        let layout = std::alloc::Layout::new::<[usize; 2]>()
            .extend(std::alloc::Layout::new::<FdState>())
            .map_err(|_| IndexError::ResourceLimited)?
            .0
            .pad_to_align();
        scratch.reserve(
            u64::try_from(layout.size()).map_err(|_| IndexError::ResourceLimited)?,
            budget,
            MemoryLifetime::Temporary,
        )?;
        Ok(Self {
            budget,
            memory,
            scratch,
            state: Rc::new(FdState {
                active: Cell::new(0),
                close_uncertain: Cell::new(false),
                max: max_handles,
                recovery_fault: Some(registration.close_fault()),
            }),
        })
    }
}
impl InventoryControl for Admitted<'_> {
    type Error = ExistingFileReadFailureV1;
    type Memory = MemoryLease;
    const ADMITTED: bool = true;
    fn checkpoint(&mut self, _: &'static str) -> Result<(), Self::Error> {
        self.budget.checkpoint()?;
        self.state.require_healthy().map_err(Into::into)
    }
    fn charge(&mut self, reads: u64, bytes: u64) -> Result<(), Self::Error> {
        self.budget
            .charge(WorkUsage {
                reads,
                bytes,
                ..WorkUsage::default()
            })
            .map_err(Into::into)
    }
    fn retain(&mut self, bytes: u64) -> Result<(), Self::Error> {
        self.memory
            .reserve(bytes, self.budget, MemoryLifetime::Retained)
    }
    fn scratch(&mut self, bytes: u64) -> Result<(), Self::Error> {
        self.scratch
            .reserve(bytes, self.budget, MemoryLifetime::Temporary)
    }
    fn temporary(&mut self, bytes: u64) -> Result<MemoryLease, Self::Error> {
        self.budget
            .reserve_memory(bytes, MemoryLifetime::Temporary)?
            .ok_or(IndexError::ResourceLimited.into())
    }
    fn fd_state(&self) -> Rc<FdState> {
        Rc::clone(&self.state)
    }
}
pub(super) fn vector_slot<T, C: InventoryControl>(
    values: &mut Vec<T>,
    control: &mut C,
) -> Result<(), C::Error> {
    if values.len() == values.capacity() {
        let next = values
            .capacity()
            .checked_mul(2)
            .ok_or_else(|| inventory_error("inventory vector capacity overflow"))?
            .max(1);
        let bytes = u64::try_from(
            std::alloc::Layout::array::<T>(next)
                .map_err(|_| inventory_error("inventory vector width overflow"))?
                .size(),
        )
        .map_err(|_| inventory_error("inventory vector width overflow"))?;
        control.scratch(bytes)?;
        values
            .try_reserve_exact(next.saturating_sub(values.len()))
            .map_err(|_| inventory_error("inventory vector allocation failed"))?;
    }
    Ok(())
}
pub(super) fn entries<C: InventoryControl>(
    directory: &ScanFile,
    control: &mut C,
    visit: &mut dyn FnMut(&[u8], &mut C) -> Result<(), C::Error>,
) -> Result<(), C::Error> {
    let file = directory.file()?;
    if C::ADMITTED {
        #[cfg(target_os = "linux")]
        {
            const BUFFER_BYTES: usize = 65_536;
            let _memory = control.temporary(
                u64::try_from(BUFFER_BYTES)
                    .map_err(|_| inventory_error("inventory directory buffer width"))?,
            )?;
            let mut buffer = Vec::<u8>::new();
            buffer
                .try_reserve_exact(BUFFER_BYTES)
                .map_err(|_| inventory_error("inventory directory buffer allocation failed"))?;
            let batch = buffer
                .spare_capacity_mut()
                .get_mut(..BUFFER_BYTES)
                .ok_or_else(|| inventory_error("inventory directory buffer is undersized"))?;
            let mut entries = rustix::fs::RawDir::new(file, batch);
            loop {
                control.checkpoint("publication admitted inventory entry")?;
                if entries.is_buffer_empty() {
                    control.charge(
                        1,
                        u64::try_from(BUFFER_BYTES)
                            .map_err(|_| inventory_error("inventory directory buffer width"))?,
                    )?;
                }
                let Some(entry) = entries.next() else {
                    break;
                };
                let entry = entry.map_err(|error| {
                    inventory_error(format!("read inventory directory: {error}"))
                })?;
                visit(entry.file_name().to_bytes(), control)?;
            }
            return Ok(());
        }
        #[cfg(not(target_os = "linux"))]
        {
            return Err(CoreError::NotImplemented(
                "admitted boot inventory requires the installed Linux getdents owner".into(),
            )
            .into());
        }
    }
    let mut entries = Dir::read_from(file)
        .map_err(|error| inventory_error(format!("enumerate inventory directory: {error}")))?;
    while let Some(entry) = entries.read() {
        control.checkpoint("publication boot inventory entry")?;
        let entry = entry.map_err(|error| inventory_error(format!("read directory: {error}")))?;
        visit(entry.file_name().to_bytes(), control)?;
    }
    Ok(())
}
pub(super) fn require_retired<C: InventoryControl>(control: &C) -> Result<(), C::Error> {
    control.fd_state().require_retired().map_err(Into::into)
}

/// Same actual canonical capacity and the original grants that funded its
/// observed inode map/ledger. Keep this value until Common retires, then drop
/// capacity before its grants. This is not a Source/publication permit.
pub(crate) struct AdmittedBootInventoryV1 {
    pub(crate) capacity: PublicationCapacity,
    pub(crate) observed_files: u64,
    pub(crate) observed_entries: u64,
    pub(crate) max_path_bytes: usize,
    pub(crate) memory: InventoryMemory,
}

#[cfg(all(test, target_os = "linux"))]
mod tests {
    use super::*;
    use crate::native_resources::{NativeResourceClass, NativeResourceLedger, NativeResourcePlan};
    use quanta_index_core::index::{
        WorkLimits,
        effort::{ReservationGuard, ResourceNeed},
    };
    use std::sync::Arc;
    type TestResult = Result<(), Box<dyn std::error::Error>>;
    fn require(value: bool, message: &str) -> TestResult {
        if value { Ok(()) } else { Err(message.into()) }
    }
    fn limits() -> PublicationBootInventoryLimits {
        PublicationBootInventoryLimits {
            max_bytes: 1024 * 1024,
            max_files: 16,
            max_pending: 16,
            max_entries: 64,
            max_depth: 8,
            max_pending_frames: 16,
        }
    }
    fn fixture(
        run: impl FnOnce(
            &Path,
            &crate::publication_file_capacity::RecoveryReadRootRegistrationV1,
            &NativeResourceLedger,
            &mut WorkBudget,
        ) -> TestResult,
    ) -> TestResult {
        let temporary = tempfile::tempdir()?;
        let lease = StateRootLease::acquire(&temporary.path().join("state"))?;
        let root = lease.state_root_identity_v1().to_path_buf();
        std::fs::create_dir_all(root.join("nested"))?;
        std::fs::write(root.join("a"), b"abc")?;
        std::fs::write(root.join("nested/b"), b"12345")?;
        let unit = ResourceNeed {
            cpu_units: 1,
            io_bytes: 8 * 1024 * 1024,
            memory_bytes: 8 * 1024 * 1024,
            provider_units: 0,
        };
        let plan = NativeResourcePlan {
            capacity: ResourceNeed {
                cpu_units: 4,
                io_bytes: 64 * 1024 * 1024,
                memory_bytes: 64 * 1024 * 1024,
                provider_units: 0,
            },
            control: unit,
            interactive: unit,
            core: unit,
            enrichment: unit,
        };
        let ledger = NativeResourceLedger::new(plan)?;
        let control = ReservationGuard::try_acquire(
            ledger.scheduler(NativeResourceClass::Control),
            plan.control,
        )
        .ok_or(IndexError::ResourceLimited)?;
        let mut budget = WorkBudget::with_request(
            WorkLimits {
                reads: 4096,
                writes: 0,
                bytes: 8 * 1024 * 1024,
                mutations: 0,
            },
            RequestBudgetV1::for_duration(std::time::Duration::from_secs(30)),
        );
        budget.bind_memory_scope(control.memory_scope());
        let memory = crate::publication_file_capacity::RecoveryReadMemoryV1::new(&root, &budget)?;
        let registration = crate::publication_file_capacity::RecoveryReadRootRegistrationV1::begin(
            lease, 256, memory,
        )?;
        run(&root, &registration, &ledger, &mut budget)?;
        drop(registration);
        drop(control);
        Ok(())
    }
    #[test]
    fn admitted_boot_inventory_repeats_with_fixed_inode_lengths_and_original_memory() -> TestResult
    {
        fixture(|root, registration, ledger, budget| {
            let baseline = ledger.snapshot()?.live_memory_bytes;
            for _ in 0..2 {
                let admitted =
                    seed_publication_capacity_admitted_v1(registration, limits(), &[], 16, budget)?;
                let observed = admitted.capacity.snapshot()?;
                // The canonical lease lock is one empty inode; two immutable
                // fixture files hold the independently fixed total of 8 bytes.
                require(
                    observed.used_bytes == 8
                        && observed.used_files == 3
                        && admitted.observed_files == 3,
                    "canonical inventory lost fixed files or reused a directory cursor",
                )?;
                require(
                    ledger.snapshot()?.live_memory_bytes > baseline,
                    "inventory ledger allocations escaped their Original grants",
                )?;
                require(
                    admitted.max_path_bytes >= root.join("nested/b").as_os_str().len(),
                    "inventory path envelope omitted an observed path",
                )?;
                drop(admitted);
                require(
                    ledger.snapshot()?.live_memory_bytes == baseline,
                    "inventory custody did not release original allocations",
                )?;
            }
            Ok(())
        })
    }
    #[test]
    fn admitted_boot_inventory_refuses_missing_memory_and_original_cancellation() -> TestResult {
        fixture(|root, registration, _ledger, budget| {
            let mut unfunded = WorkBudget::with_request(
                WorkLimits {
                    reads: 4096,
                    writes: 0,
                    bytes: 8 * 1024 * 1024,
                    mutations: 0,
                },
                budget.request().clone(),
            );
            require(
                seed_publication_capacity_admitted_v1(
                    registration,
                    limits(),
                    &[],
                    16,
                    &mut unfunded,
                )
                .is_err(),
                "unfunded inventory constructed physical capacity",
            )?;
            require(
                unfunded.usage().reads == 0,
                "unfunded inventory performed namespace IO",
            )?;
            budget.request().cancel_handle().cancel();
            require(
                seed_publication_capacity_admitted_v1(registration, limits(), &[], 16, budget)
                    .is_err(),
                "cancelled inventory constructed physical capacity",
            )?;
            require(
                std::fs::read(root.join("a"))? == b"abc"
                    && std::fs::read(root.join("nested/b"))? == b"12345",
                "refused inventory modified input files",
            )
        })
    }
    #[test]
    fn admitted_boot_inventory_fd_refusal_retires_all_original_scan_descriptors() -> TestResult {
        fixture(|root, registration, _ledger, budget| {
            let mut control = Admitted::new(budget, 1, registration)?;
            let lease = registration.lease()?;
            let result = seed_with_control(lease, root, limits(), &[], &mut control);
            require(
                result.is_err(),
                "one-handle inventory bypassed descriptor admission",
            )?;
            require_retired(&control)?;
            lease.require_state_root_v1(root)?;
            Ok(())
        })
    }
    #[test]
    fn admitted_boot_inventory_poison_refuses_next_open_and_checkpoint_without_io() -> TestResult {
        fixture(|_root, registration, _ledger, budget| {
            let mut control = Admitted::new(budget, 16, registration)?;
            let state = control.fd_state();
            state.mark_close_uncertain();
            let mut opened = false;
            let result = ScanFile::open(&mut control, || {
                opened = true;
                Err(inventory_error("poisoned opener must not run"))
            });
            require(
                result.is_err() && !opened,
                "poisoned census attempted a new namespace open",
            )?;
            require(
                control.checkpoint("poison law").is_err(),
                "poisoned census continued to next entry",
            )?;
            Ok(())
        })
    }
    #[test]
    fn admitted_boot_inventory_rejects_foreign_root_memory_before_registration() -> TestResult {
        fixture(|root, registration, _ledger, budget| {
            let foreign = root.join("different-retained-root");
            let memory =
                crate::publication_file_capacity::RecoveryReadMemoryV1::new(&foreign, budget)?;
            let result = crate::publication_file_capacity::RecoveryReadRootRegistrationV1::begin(
                registration.lease()?.clone(),
                256,
                memory,
            );
            require(
                matches!(result,Err(CoreError::InvalidContract(ref detail)) if detail.contains("reserved for another root")),
                "foreign retained backing reached the active-root registry",
            )
        })
    }
    #[test]
    fn admitted_boot_inventory_failed_registration_cannot_reuse_original_backing() -> TestResult {
        fixture(|root, registration, _ledger, budget| {
            let memory = crate::publication_file_capacity::RecoveryReadMemoryV1::new(root, budget)?;
            let first = crate::publication_file_capacity::RecoveryReadRootRegistrationV1::begin(
                registration.lease()?.clone(),
                256,
                Arc::clone(&memory),
            );
            require(
                matches!(first,Err(CoreError::InvalidContract(ref detail)) if detail.contains("already has an active")),
                "first attempt did not encounter the actual held binder",
            )?;
            let second = crate::publication_file_capacity::RecoveryReadRootRegistrationV1::begin(
                registration.lease()?.clone(),
                256,
                memory,
            );
            require(
                matches!(second,Err(CoreError::InvalidContract(ref detail)) if detail.contains("registration already started")),
                "failed registration backing was reused by another attempt",
            )
        })
    }
}
