//! Existing ROOT_CUSTODY lifecycle for one actually inventoried recovery reader.
//! These guards do not issue Source or publication permission.
use super::*;
use quanta_index_core::index::{
    IndexError, MemoryLease, MemoryLifetime, WorkBudget, existing_file::ExistingFileReadFailureV1,
};
use std::sync::atomic::{AtomicBool, Ordering};

/// Memory grants retained through inventory and recovered reader custody.
pub(crate) struct InventoryMemory {
    grants: Vec<MemoryLease>,
}
impl InventoryMemory {
    pub(crate) fn new() -> Self {
        Self { grants: Vec::new() }
    }
    pub(crate) fn reserve(
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

pub(crate) struct RecoveryReadMemoryV1 {
    root: PathBuf,
    registration_started: AtomicBool,
    inventory: Mutex<Option<InventoryMemory>>,
    binder: Mutex<Option<MemoryLease>>,
    _initial: MemoryLease,
}
impl RecoveryReadMemoryV1 {
    pub(crate) fn new(root: &Path, budget: &WorkBudget) -> Result<Arc<Self>, IndexError> {
        let bytes = PublicationFileCapacity::recovery_root_allocation_bytes_v1(root)?;
        let grant = budget
            .reserve_memory(bytes, MemoryLifetime::Retained)?
            .ok_or(IndexError::ResourceLimited)?;
        Ok(Arc::new(Self {
            root: root.to_path_buf(),
            registration_started: AtomicBool::new(false),
            _initial: grant,
            inventory: Mutex::new(None),
            binder: Mutex::new(None),
        }))
    }
    pub(crate) fn hold_inventory(&self, inventory: InventoryMemory) -> Result<(), CoreError> {
        let mut target = lock(&self.inventory)?;
        if target.is_some() {
            return Err(contract("recovery inventory memory already installed"));
        }
        *target = Some(inventory);
        Ok(())
    }
    pub(crate) fn hold_binder(&self, memory: MemoryLease) -> Result<(), CoreError> {
        let mut target = lock(&self.binder)?;
        if target.is_some() {
            return Err(contract("recovery binder memory already installed"));
        }
        *target = Some(memory);
        Ok(())
    }
}
#[derive(Clone)]
pub(crate) struct RecoveryReadCloseFaultV1(Arc<AtomicBool>);
impl RecoveryReadCloseFaultV1 {
    pub(crate) fn mark(&self) {
        self.0.store(true, Ordering::SeqCst);
    }
}
pub(crate) struct RecoveryReadRootRegistrationV1 {
    lease: Option<StateRootLease>,
    close_fault: RecoveryReadCloseFaultV1,
    memory: Arc<RecoveryReadMemoryV1>,
}
impl RecoveryReadRootRegistrationV1 {
    pub(crate) fn begin(
        lease: StateRootLease,
        requested_roots: usize,
        memory: Arc<RecoveryReadMemoryV1>,
    ) -> Result<Self, CoreError> {
        if requested_roots == 0 {
            return Err(contract("recovery process-root ceiling is zero"));
        }
        if memory.root.as_path() != lease.state_root_identity_v1() {
            return Err(contract("recovery memory was reserved for another root"));
        }
        // One original retained backing cannot fund two registry/binder owners.
        // Failure consumes this attempt; a retry obtains its own real grant.
        if memory
            .registration_started
            .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
            .is_err()
        {
            return Err(contract("recovery memory registration already started"));
        }
        lease.require_state_root_v1(lease.state_root_identity_v1())?;
        // The already-live process pool can be stricter than this saved policy;
        // never raise it or fabricate a second global root registry.
        let actual_limit = if let Some(registry) = ROOT_CUSTODY.get() {
            let state = lock(registry)?;
            if state.limit > requested_roots {
                return Err(capacity(
                    "saved recovery process-root bound below actual global pool",
                ));
            }
            state.limit
        } else {
            requested_roots
        };
        register_root_custody(lease.state_root_identity_v1(), actual_limit)?;
        Ok(Self {
            lease: Some(lease),
            memory,
            close_fault: RecoveryReadCloseFaultV1(Arc::new(AtomicBool::new(false))),
        })
    }
    pub(crate) fn lease(&self) -> Result<&StateRootLease, CoreError> {
        self.lease
            .as_ref()
            .ok_or_else(|| contract("recovery root registration already transferred"))
    }
    pub(crate) fn close_fault(&self) -> RecoveryReadCloseFaultV1 {
        self.close_fault.clone()
    }
    pub(super) fn transfer(
        mut self,
        lease: &StateRootLease,
    ) -> Result<Arc<RecoveryReadMemoryV1>, CoreError> {
        let held = self.lease()?;
        held.require_state_root_v1(lease.state_root_identity_v1())?;
        lease.require_state_root_v1(held.state_root_identity_v1())?;
        if self.close_fault.0.load(Ordering::SeqCst) {
            return Err(contract("recovery inventory close was not witnessed"));
        }
        // The real registration, not a caller bool, is consumed by Common.
        drop(self.lease.take());
        Ok(Arc::clone(&self.memory))
    }
}
impl Drop for RecoveryReadRootRegistrationV1 {
    fn drop(&mut self) {
        let Some(lease) = self.lease.take() else {
            return;
        };
        let Ok(mut registry) = custody_registry_or_abort().lock() else {
            std::process::abort();
        };
        let root = lease.state_root_identity_v1();
        let Some(entry) = registry.roots.get_mut(root) else {
            std::process::abort();
        };
        if self.close_fault.0.load(Ordering::SeqCst) && entry.quarantine.is_none() {
            entry.quarantine = Some(lease.clone());
            entry.recovery_memory = Some(Arc::clone(&self.memory));
        }
        let Some(next) = entry.active.checked_sub(1) else {
            std::process::abort();
        };
        entry.active = next;
        if next == 0 && entry.quarantine.is_none() {
            drop(registry.roots.remove(root));
        }
        // The real root lease and original retained allocation survive an
        // ambiguous scan close in the existing process quarantine entry.
    }
}
fn arc_layout<T>() -> Result<u64, CoreError> {
    let layout = MemoryLease::shared_allocation_backing_layout(std::alloc::Layout::new::<T>())
        .map_err(|_| contract("recovery Arc layout overflow"))?;
    u64::try_from(layout.size()).map_err(|_| contract("recovery Arc layout width overflow"))
}
impl PublicationFileCapacity {
    pub(crate) fn recovery_root_allocation_bytes_v1(root: &Path) -> Result<u64, IndexError> {
        let bytes = (|| -> Result<u64, CoreError> {
            let node =
                quanta_index_core::publication_capacity::publication_inventory_map_node_bytes_v1::<
                    PathBuf,
                    RootCustodyEntry,
                >()?;
            let memory = arc_layout::<RecoveryReadMemoryV1>()?;
            let marker = arc_layout::<AtomicBool>()?;
            let files = arc_layout::<File>()?
                .checked_mul(2)
                .ok_or_else(|| contract("recovery lease backing overflow"))?;
            let mut bytes = node
                .checked_add(memory)
                .and_then(|n| n.checked_add(marker))
                .and_then(|n| n.checked_add(files))
                .ok_or_else(|| contract("recovery root backing sum overflow"))?;
            // Root/LOCK/private path traversal and the actual lease/path clones
            // overlap during acquisition, Common construction and quarantine.
            bytes = bytes
                .checked_add(
                    u64::try_from(root.as_os_str().len())
                        .map_err(|_| contract("recovery path width"))?
                        .checked_mul(16)
                        .ok_or_else(|| contract("recovery path backing overflow"))?,
                )
                .ok_or_else(|| contract("recovery root/path sum overflow"))?;
            Ok(bytes)
        })();
        bytes.map_err(|_| IndexError::ResourceLimited)
    }
    pub(crate) fn recovery_binder_allocation_bytes_v1(
        files: u64,
        entries: u64,
        max_path: usize,
    ) -> Result<u64, CoreError> {
        let inode =
            quanta_index_core::publication_capacity::publication_inventory_map_node_bytes_v1::<
                InodeKey,
                InodeState,
            >()?;
        let opening =
            quanta_index_core::publication_capacity::publication_inventory_map_node_bytes_v1::<
                PathBuf,
                usize,
            >()?;
        let paths = u64::try_from(max_path)
            .map_err(|_| contract("recovery path width"))?
            .checked_mul(8)
            .ok_or_else(|| contract("recovery path backing overflow"))?;
        let per_open = opening
            .checked_add(arc_layout::<Anchor>()?)
            .and_then(|n| n.checked_add(paths))
            .ok_or_else(|| contract("recovery opening backing overflow"))?;
        let shared = arc_layout::<Shared>()?;
        let facade = arc_layout::<Self>()?;
        files
            .checked_mul(inode)
            .and_then(|n| n.checked_add(entries.checked_mul(per_open)?))
            .and_then(|n| n.checked_add(shared))
            .and_then(|n| n.checked_add(facade))
            .ok_or_else(|| contract("recovery binder backing overflow"))
    }
    pub(crate) fn from_recovery_inventory_v1(
        registration: RecoveryReadRootRegistrationV1,
        authority: PublicationCapacity,
        max_handles: usize,
        max_entries: usize,
        max_roots: usize,
    ) -> Result<Arc<Self>, CoreError> {
        let lease = registration.lease()?.clone();
        Self::new_internal(
            lease,
            authority,
            max_handles,
            max_entries,
            max_roots,
            None,
            Some(registration),
        )
    }

    /// Keep the normal Native maintenance reserve while transferring an
    /// admitted recovery inventory and its original root registration.
    pub(crate) fn from_recovery_inventory_with_maintenance_v1(
        registration: RecoveryReadRootRegistrationV1,
        authority: PublicationCapacity,
        max_handles: usize,
        max_entries: usize,
        max_roots: usize,
        maintenance_bytes: u64,
        maintenance_files: u64,
    ) -> Result<Arc<Self>, CoreError> {
        if maintenance_bytes == 0 || maintenance_files == 0 {
            return Err(contract("maintenance reserve must be positive"));
        }
        let lease = registration.lease()?.clone();
        Self::new_internal(
            lease,
            authority,
            max_handles,
            max_entries,
            max_roots,
            Some((maintenance_bytes, maintenance_files)),
            Some(registration),
        )
    }
}
