//! Read facets from one saved Legacy policy and an existing offline root.
//! This owner supplies physical custody only; it cannot issue Source permission.
use super::*;
use crate::{
    deployment_selection::DeploymentSelectionDocumentV1,
    publication_file_capacity::{
        PublicationFileCapacity, RecoveryReadMemoryV1, RecoveryReadRootRegistrationV1,
    },
    publication_inventory::{
        PublicationBootInventoryLimits, seed_publication_capacity_admitted_v1,
    },
    publication_sqlite_vfs::SqliteCapacityVfs,
};
use quanta_index_contract::{
    native_cutover_evidence_v1::NativeCutoverDigest64V1,
    product_root_locator_v2::{
        PRODUCT_ROOT_IDENTITY_BYTES_V2, PRODUCT_ROOT_IDENTITY_FILE_V2, ProductRootKindV2,
        decode_product_root_locator_v2,
    },
};
use quanta_index_core::{
    ExistingTreeMetadataIoPortV1,
    index::{MemoryLifetime, WorkUsage},
};
use std::{ffi::CString, path::Path};

pub(super) struct LegacyReadAuthorityV1 {
    // This extra lease clone drops while Common still owns its funded root.
    lease: StateRootLease,
    catalog: SqliteCapacityVfs,
    objects: NativeObjectReadIo,
    metadata: crate::publication_file_capacity::ExistingTreeReadIoV1,
    physical: Arc<PublicationFileCapacity>,
    observed_files: usize,
    repomap_candidate_file_bytes: u64,
}
impl LegacyReadAuthorityV1 {
    pub(super) fn acquire(
        selection: &DeploymentSelectionDocumentV1,
        native_io: &crate::native_scratch_io::NativeScratchIo,
        lease: StateRootLease,
        memory: Arc<RecoveryReadMemoryV1>,
        budget: &mut WorkBudget,
    ) -> Result<Self> {
        Self::acquire_inner(selection, Some(native_io), lease, memory, budget)
    }

    /// The exporter is a separate read-only child, so no candidate-root VFS
    /// exists in its process. Register the same saved Legacy physical policy
    /// against its actual held root without introducing a writable store.
    pub(super) fn acquire_for_checkpoint_export_v1(
        selection: &DeploymentSelectionDocumentV1,
        lease: StateRootLease,
        memory: Arc<RecoveryReadMemoryV1>,
        budget: &mut WorkBudget,
    ) -> Result<Self> {
        Self::acquire_inner(selection, None, lease, memory, budget)
    }

    fn acquire_inner(
        selection: &DeploymentSelectionDocumentV1,
        native_io: Option<&crate::native_scratch_io::NativeScratchIo>,
        lease: StateRootLease,
        memory: Arc<RecoveryReadMemoryV1>,
        budget: &mut WorkBudget,
    ) -> Result<Self> {
        let policy = selection.legacy_physical_policy_v1(budget)?;
        let root = selection.state_root_v1();
        lease.require_state_root_v1(root)?;
        let registration = RecoveryReadRootRegistrationV1::begin(
            lease,
            usize::try_from(policy.binder.process_roots)?,
            Arc::clone(&memory),
        )?;
        let sockets =
            quanta_index_searchd::app::config::SearchdConfig::socket_paths_for_state_root_v1(root);
        let inventory = seed_publication_capacity_admitted_v1(
            &registration,
            PublicationBootInventoryLimits {
                max_bytes: policy.inventory.bytes,
                max_files: policy.inventory.files,
                max_pending: policy.inventory.pending,
                max_entries: policy.inventory.entries,
                max_depth: policy.inventory.depth,
                max_pending_frames: policy.inventory.pending_frames,
            },
            &sockets,
            usize::try_from(policy.binder.reader_handles)?,
            budget,
        )?;
        let entries = inventory
            .observed_entries
            .checked_add(1)
            .ok_or(IndexError::ResourceLimited)?;
        // Read-only facades cannot introduce new names. Clamp the real local
        // handle/entry bounds to this observed tree before sizing their maps.
        // A tiny tree can refuse a multi-handle SQLite open; it never raises a
        // saved policy or guesses a deployment default to make recovery pass.
        let inherited_handles = match native_io {
            Some(native_io) => {
                native_io.recovery_reader_handle_ceiling_v1(policy.global_sqlite_handles)?
            }
            None => usize::try_from(policy.global_sqlite_handles)?,
        };
        let handles = usize::try_from(policy.binder.reader_handles)?
            .min(usize::try_from(entries)?)
            .min(inherited_handles);
        let tracked =
            usize::try_from(policy.binder.tracked_entries)?.min(usize::try_from(entries)?);
        let common_bytes = PublicationFileCapacity::recovery_binder_allocation_bytes_v1(
            inventory.observed_files,
            entries,
            inventory.max_path_bytes,
        )?;
        let vfs_bytes = match native_io {
            Some(native_io) => {
                native_io.recovery_sibling_allocation_bytes_v1(entries, inventory.max_path_bytes)?
            }
            None => SqliteCapacityVfs::initial_recovery_allocation_bytes_v1(
                entries,
                inventory.max_path_bytes,
                "quanta-publication-capacity".len(),
            )?,
        };
        let backing = common_bytes
            .checked_add(vfs_bytes)
            .ok_or(IndexError::ResourceLimited)?;
        let binder = budget
            .reserve_memory(backing, MemoryLifetime::Retained)?
            .ok_or(IndexError::ResourceLimited)?;
        memory.hold_binder(binder)?;
        memory.hold_inventory(inventory.memory)?;
        let lease = registration.lease()?.clone();
        let physical = PublicationFileCapacity::from_recovery_inventory_v1(
            registration,
            inventory.capacity,
            handles,
            tracked,
            usize::try_from(policy.binder.process_roots)?,
        )?;
        let catalog = match native_io {
            Some(native_io) => native_io.register_recovery_sibling_v1(
                Arc::clone(&physical),
                policy.global_sqlite_handles,
                budget,
            )?,
            None => SqliteCapacityVfs::register_funded(
                Arc::clone(&physical),
                CString::new("quanta-publication-capacity")?,
                policy.global_sqlite_handles,
            )?,
        };
        let objects = NativeObjectReadIo::new(Arc::clone(&physical));
        let metadata = physical.existing_tree_read_io_v1();
        Ok(Self {
            catalog,
            objects,
            metadata,
            physical,
            lease,
            observed_files: usize::try_from(inventory.observed_files)?,
            repomap_candidate_file_bytes: policy.repomap_candidate_file_bytes,
        })
    }
    pub(super) fn root(&self) -> &Path {
        self.lease.state_root_identity_v1()
    }
    pub(super) fn require_current_root_v1(&self) -> Result<()> {
        self.lease.require_state_root_v1(self.root())?;
        let _observed = self.lease.observe_current_root_inode_v1()?;
        self.physical.validate_current_state_root()?;
        Ok(())
    }
    pub(super) fn catalog(&self) -> &dyn quanta_index_catalog::CatalogReadOnlyIoAdmission {
        &self.catalog
    }
    pub(super) fn objects(&self) -> &dyn ObjectReadIoPort {
        &self.objects
    }
    pub(super) fn metadata(&self) -> &dyn ExistingTreeMetadataIoPortV1 {
        &self.metadata
    }
    pub(super) fn maximum_objects(&self) -> usize {
        self.observed_files
    }

    pub(super) fn repomap_candidate_file_bytes_v1(&self) -> u64 {
        self.repomap_candidate_file_bytes
    }

    pub(super) fn require_repomap_locator_v1(
        &self,
        expected: NativeCutoverDigest64V1,
        budget: &mut WorkBudget,
    ) -> Result<()> {
        let root = self.root();
        let depth = u64::try_from(root.components().count())?;
        budget.charge(WorkUsage {
            reads: depth
                .checked_mul(32)
                .and_then(|n| n.checked_add(64))
                .ok_or(IndexError::ResourceLimited)?,
            bytes: 512,
            ..WorkUsage::default()
        })?;
        let _path_memory = budget
            .reserve_memory(
                u64::try_from(root.as_os_str().len())?
                    .checked_mul(4)
                    .and_then(|n| n.checked_add(512))
                    .ok_or(IndexError::ResourceLimited)?,
                MemoryLifetime::Temporary,
            )?
            .ok_or(IndexError::ResourceLimited)?;
        let held = self.lease.observe_current_root_inode_v1()?;
        let marker = quanta_index_core::index::existing_file::read_existing_private_file_bytes_v1(
            &root.join(PRODUCT_ROOT_IDENTITY_FILE_V2),
            rustix::process::geteuid().as_raw(),
            u64::try_from(PRODUCT_ROOT_IDENTITY_BYTES_V2)?,
            self.objects(),
            budget,
        )?;
        self.lease.require_state_root_v1(root)?;
        let observed = decode_product_root_locator_v2(
            marker.bytes_v1(),
            ProductRootKindV2::RepoMap,
            held.device,
            held.inode,
        );
        if observed != Some(expected.0) {
            return Err(IndexError::DigestConflict.into());
        }
        Ok(())
    }

    pub(super) fn verify_active_repomap_v1(
        &self,
        quiescence: &quanta_index_catalog::LegacyCatalogQuiescenceV1,
        receipts: (
            &quanta_index_contract::RepoMapTerminalReceiptV2,
            &quanta_index_contract::RepoMapTerminalReceiptV2,
        ),
        budget: &mut WorkBudget,
    ) -> Result<quanta_index_repomap::VerifiedActiveRepoMapFilesV1> {
        Ok(
            quanta_index_repomap::RepoMapGenerationStore::verify_existing_active_publication_v1(
                self.root(),
                quiescence,
                receipts,
                rustix::process::geteuid().as_raw(),
                self.repomap_candidate_file_bytes,
                &self.objects,
                budget,
            )?,
        )
    }
    /// Lend one Catalog-selected Legacy RepoMap compiled body while the saved
    /// root, its physical read port and the caller's WorkBudget remain held.
    pub(super) fn visit_active_repomap_compiled_v1(
        &self,
        candidate: &quanta_index_catalog::RepoMapCandidateRowV1,
        receipts: Option<(
            &quanta_index_contract::RepoMapTerminalReceiptV2,
            &quanta_index_contract::RepoMapTerminalReceiptV2,
        )>,
        budget: &mut WorkBudget,
        visit: &mut dyn FnMut(
            &[u8],
            &mut WorkBudget,
        ) -> std::result::Result<
            (),
            quanta_index_core::index::existing_file::ExistingFileReadFailureV1,
        >,
    ) -> Result<quanta_index_repomap::VerifiedActiveRepoMapFilesV1> {
        self.require_current_root_v1()?;
        let files = quanta_index_repomap::visit_existing_active_repomap_compiled_payload_v1(
            self.root(),
            candidate,
            receipts,
            rustix::process::geteuid().as_raw(),
            self.repomap_candidate_file_bytes,
            &self.objects,
            budget,
            visit,
        )?;
        self.require_current_root_v1()?;
        Ok(files)
    }
    pub(super) fn require_current_and_retired(&self, budget: &mut WorkBudget) -> Result<()> {
        budget.charge(WorkUsage {
            reads: u64::try_from(self.root().components().count())?
                .checked_mul(16)
                .and_then(|n| n.checked_add(64))
                .ok_or(IndexError::ResourceLimited)?,
            bytes: u64::try_from(self.observed_files)?
                .checked_mul(u64::try_from(std::mem::size_of::<
                    quanta_index_core::publication_capacity::InodeKey,
                >())?)
                .ok_or(IndexError::ResourceLimited)?,
            ..WorkUsage::default()
        })?;
        self.catalog.drain_retirements()?;
        self.physical.sweep_retired()?;
        self.physical
            .require_quiescent("Legacy offline recovery read facets")?;
        self.lease.require_state_root_v1(self.root())?;
        Ok(())
    }
    pub(super) fn retire_catalog(&self, budget: &mut WorkBudget) -> Result<()> {
        budget.checkpoint()?;
        self.catalog.retire_checked()?;
        self.require_current_and_retired(budget)
    }
}
