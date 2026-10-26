//! One leased-root physical authority for the Legacy search-plane composition.

use std::ffi::CString;
use std::path::PathBuf;
use std::sync::Arc;

use quanta_index_core::publication_admission::CatalogPublicationFundingPort;
use quanta_index_core::{CoreError, RequestBudgetV1};
use quanta_index_ipc::SOURCE_PUBLICATION_UPLOAD_STAGING_SLOTS;
use quanta_index_searchd::app::config::LegacyPublicationPhysicalPolicy;
use quanta_index_searchd::app::runtime::StateRootLease;

use crate::canonical_publication_admission::{
    CanonicalPublicationAdmission, PublicationAllowanceLimit,
};
use crate::durable_control_admission::CanonicalDurableControlFileAdmission;
use crate::history_text_file_io::CanonicalHistoryTextFileAdmissionFactory;
use crate::publication_file_capacity::PublicationFileCapacity;
use crate::publication_file_deletions::PublicationFileDeletions;
use crate::publication_inventory::{PublicationBootInventoryLimits, seed_publication_capacity};
use crate::publication_sqlite_vfs::{ServiceFundedCatalogIo, SqliteCapacityVfs};
use crate::repomap_file_admission::CanonicalRepoMapFileAdmission;
use crate::semantic_file_factory::{
    CanonicalSemanticFileAdmissionFactory, CanonicalSemanticMaintenanceFileFactory,
};
use crate::source_upload_file_admission::CanonicalSourceUploadFileAdmission;

fn contract(message: impl Into<String>) -> CoreError {
    CoreError::InvalidContract(format!("Legacy physical assembly: {}", message.into()))
}

fn checked_usize(value: u64, name: &str) -> Result<usize, CoreError> {
    usize::try_from(value).map_err(|error| contract(format!("{name} exceeds usize: {error}")))
}

/// All ports share one binder and one retained pending-deletion owner.
///
/// The Catalog VFS is registered, but no Catalog connection is opened here.
pub(crate) struct LegacyPhysicalAssembly {
    pub(crate) state_root_lease: StateRootLease,
    pub(crate) publication: Arc<CanonicalPublicationAdmission>,
    pub(crate) durable_control: Arc<CanonicalDurableControlFileAdmission>,
    pub(crate) source_upload: Arc<CanonicalSourceUploadFileAdmission>,
    pub(crate) repomap: Arc<CanonicalRepoMapFileAdmission>,
    pub(crate) history: Arc<CanonicalHistoryTextFileAdmissionFactory>,
    pub(crate) semantic: Arc<CanonicalSemanticFileAdmissionFactory>,
    pub(crate) semantic_maintenance: Arc<CanonicalSemanticMaintenanceFileFactory>,
    pub(crate) catalog_funding: Arc<dyn CatalogPublicationFundingPort>,
    pub(crate) catalog_io: Arc<ServiceFundedCatalogIo>,
}

/// Seeded, leased physical authority before any domain adapter is constructed.
///
/// A staged-root producer may write through this binder before mutable
/// Catalog, `RepoMap`, or index adapters inspect the produced image.
pub(crate) struct LegacyPhysicalAuthority {
    state_root_lease: StateRootLease,
    policy: LegacyPublicationPhysicalPolicy,
    source_body_max_bytes: u64,
    scan_entries: usize,
    binder: Arc<PublicationFileCapacity>,
    deletions: Arc<PublicationFileDeletions>,
    publication: Arc<CanonicalPublicationAdmission>,
    durable_control: Arc<CanonicalDurableControlFileAdmission>,
    catalog_funding: Arc<dyn CatalogPublicationFundingPort>,
    catalog_io: Option<Arc<ServiceFundedCatalogIo>>,
    maintenance_retired: bool,
    retired: bool,
}

impl LegacyPhysicalAuthority {
    /// `allowed_socket_entries` is the exact configured service socket set.
    pub(crate) fn new(
        state_root_lease: StateRootLease,
        policy: LegacyPublicationPhysicalPolicy,
        source_body_max_bytes: u64,
        boot_budget: &RequestBudgetV1,
        allowed_socket_entries: &[PathBuf],
    ) -> Result<Self, CoreError> {
        let policy = policy
            .validate()
            .map_err(|error| contract(format!("invalid configured policy: {error}")))?;
        if source_body_max_bytes == 0 || source_body_max_bytes > policy.inventory.bytes {
            return Err(contract(
                "source body maximum must be positive and within the global byte cap",
            ));
        }
        let scan_entries = checked_usize(policy.inventory.entries, "inventory entries")?;
        let max_open_handles = usize::try_from(policy.binder.reader_handles)
            .map_err(|error| contract(format!("reader handles exceed usize: {error}")))?;
        let max_tracked_entries = usize::try_from(policy.binder.tracked_entries)
            .map_err(|error| contract(format!("tracked entries exceed usize: {error}")))?;
        let max_process_roots = usize::try_from(policy.binder.process_roots)
            .map_err(|error| contract(format!("process roots exceed usize: {error}")))?;

        let state_root = state_root_lease.state_root_identity_v1();
        let inventory = seed_publication_capacity(
            &state_root_lease,
            state_root,
            PublicationBootInventoryLimits {
                max_bytes: policy.inventory.bytes,
                max_files: policy.inventory.files,
                max_pending: policy.inventory.pending,
                max_entries: policy.inventory.entries,
                max_depth: policy.inventory.depth,
                max_pending_frames: policy.inventory.pending_frames,
            },
            allowed_socket_entries,
            boot_budget,
        )?;
        let binder = PublicationFileCapacity::new_with_maintenance(
            state_root_lease.clone(),
            inventory,
            max_open_handles,
            max_tracked_entries,
            max_process_roots,
            policy.maintenance.bytes,
            policy.maintenance.files,
        )?;
        let deletions = Arc::new(PublicationFileDeletions::new(Arc::clone(&binder)));

        let publication = Arc::new(CanonicalPublicationAdmission::new(
            Arc::clone(&deletions),
            PublicationAllowanceLimit::new(policy.control.bytes, policy.control.files)?,
            PublicationAllowanceLimit::new(policy.work.bytes, policy.work.files)?,
        ));
        let durable_control = Arc::new(CanonicalDurableControlFileAdmission::new(
            Arc::clone(&deletions),
            policy.durable_control_file_bytes,
        )?);
        let catalog_vfs = Arc::new(SqliteCapacityVfs::register_funded(
            Arc::clone(&binder),
            CString::new("quanta-publication-capacity")
                .map_err(|error| contract(format!("SQLite VFS name contains NUL: {error}")))?,
            policy.global_sqlite_handles,
        )?);
        let catalog_funding: Arc<dyn CatalogPublicationFundingPort> = catalog_vfs.clone();
        let catalog_parent = binder.reserve_publication_allowance(
            policy.catalog_parent.bytes,
            policy.catalog_parent.files,
        )?;
        let catalog_io = Arc::new(ServiceFundedCatalogIo::new(
            Arc::clone(&catalog_vfs),
            Arc::clone(&catalog_parent),
        )?);

        Ok(Self {
            state_root_lease,
            policy,
            source_body_max_bytes,
            scan_entries,
            binder,
            deletions,
            publication,
            durable_control,
            catalog_funding,
            catalog_io: Some(catalog_io),
            maintenance_retired: false,
            retired: false,
        })
    }

    #[must_use]
    pub(crate) fn binder(&self) -> Arc<PublicationFileCapacity> {
        Arc::clone(&self.binder)
    }

    #[must_use]
    pub(crate) fn deletions(&self) -> Arc<PublicationFileDeletions> {
        Arc::clone(&self.deletions)
    }

    #[must_use]
    pub(crate) fn state_root_lease(&self) -> &StateRootLease {
        &self.state_root_lease
    }

    pub(crate) fn catalog_io(&self) -> Result<Arc<ServiceFundedCatalogIo>, CoreError> {
        self.catalog_io
            .as_ref()
            .map(Arc::clone)
            .ok_or_else(|| contract("Catalog IO was retired"))
    }

    /// Build the existing adapters only after an offline producer has finished
    /// its copied-file phase. Every port shares the previously seeded ledger.
    pub(crate) fn build_domains(&self) -> Result<LegacyPhysicalAssembly, CoreError> {
        if self.retired {
            return Err(contract("physical authority already retired"));
        }
        self.binder.validate_current_state_root()?;
        self.catalog_io()?.drain_retirements()?;
        self.binder
            .require_quiescent("offline physical domain construction")?;
        let source_upload = Arc::new(CanonicalSourceUploadFileAdmission::new(
            Arc::clone(&self.deletions),
            self.state_root_lease.try_clone_root_directory_v1()?,
            self.source_body_max_bytes,
            SOURCE_PUBLICATION_UPLOAD_STAGING_SLOTS,
            self.scan_entries,
        )?);
        let repomap = Arc::new(CanonicalRepoMapFileAdmission::new(
            Arc::clone(&self.deletions),
            self.state_root_lease.try_clone_root_directory_v1()?,
            self.policy.repomap_control_file_bytes,
            self.policy.repomap_candidate_file_bytes,
            self.scan_entries,
        )?);
        let history = Arc::new(CanonicalHistoryTextFileAdmissionFactory::new(
            Arc::clone(&self.deletions),
            self.state_root_lease.try_clone_root_directory_v1()?,
            self.policy.history.bytes,
            self.policy.history.files,
            self.scan_entries,
        )?);
        let semantic = Arc::new(CanonicalSemanticFileAdmissionFactory::new(Arc::clone(
            &self.deletions,
        )));
        let semantic_maintenance = Arc::new(CanonicalSemanticMaintenanceFileFactory::new(
            Arc::clone(&self.deletions),
        ));
        Ok(LegacyPhysicalAssembly {
            state_root_lease: self.state_root_lease.clone(),
            publication: Arc::clone(&self.publication),
            durable_control: Arc::clone(&self.durable_control),
            source_upload,
            repomap,
            history,
            semantic,
            semantic_maintenance,
            catalog_funding: Arc::clone(&self.catalog_funding),
            catalog_io: self.catalog_io()?,
        })
    }

    /// Keep this owner intact on failure so its parent, VFS and root lease
    /// remain available for the original pending-intent reconciliation.
    pub(crate) fn retire_checked(&mut self) -> Result<(), CoreError> {
        if self.retired {
            return Ok(());
        }
        if !self.maintenance_retired {
            if let Some(io) = self.catalog_io.as_ref() {
                io.drain_retirements()?;
            }
            self.deletions.reconcile_pending_namespace()?;
            self.deletions.reconcile_retired_control_creates()?;
            self.binder.sweep_retired()?;
            let before = self.binder.capacity_snapshot()?;
            // These are the actual stored parents: maintenance is retained by
            // the binder, and Catalog IO may already have been retired on a
            // prior attempt. No parent is subtracted from a snapshot.
            let expected_allowances = if self.catalog_io.is_some() { 2 } else { 1 };
            if !self.binder.is_quiescent()?
                || before.active_allowances != expected_allowances
                || before.active_tickets != 0
                || before.uncertain_pending != 0
                || before.poisoned
            {
                return Err(CoreError::NotReady(format!(
                    "offline physical authority has consumers before retirement: {before:?}"
                )));
            }
            if let Some(io) = self.catalog_io.as_ref() {
                if Arc::strong_count(io) != 1 {
                    return Err(CoreError::NotReady(
                        "Catalog IO consumers remain at authority retirement".into(),
                    ));
                }
                io.retire_checked()?;
            }
            drop(self.catalog_io.take());
            self.deletions.reconcile_pending_namespace()?;
            self.deletions.reconcile_retired_control_creates()?;
            self.binder.sweep_retired()?;
            self.binder.retire_maintenance_reserve_checked()?;
            self.maintenance_retired = true;
        }
        let snapshot = self.binder.capacity_snapshot()?;
        if !self.binder.is_quiescent()?
            || snapshot.active_allowances != 0
            || snapshot.active_tickets != 0
            || snapshot.uncertain_pending != 0
            || snapshot.poisoned
        {
            return Err(CoreError::NotReady(format!(
                "offline physical authority retains custody: {snapshot:?}"
            )));
        }
        self.retired = true;
        Ok(())
    }
}

impl LegacyPhysicalAssembly {
    /// Seed occupancy before opening mutable adapters or Catalog.
    pub(crate) fn new(
        state_root_lease: StateRootLease,
        policy: LegacyPublicationPhysicalPolicy,
        source_body_max_bytes: u64,
        boot_budget: &RequestBudgetV1,
        allowed_socket_entries: &[PathBuf],
    ) -> Result<Self, CoreError> {
        LegacyPhysicalAuthority::new(
            state_root_lease,
            policy,
            source_body_max_bytes,
            boot_budget,
            allowed_socket_entries,
        )?
        .build_domains()
    }
}
