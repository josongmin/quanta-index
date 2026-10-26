//! One offline destination authority from first object birth to checked cutover.

use std::fmt::Write as _;
use std::fs::{self, File};
use std::os::unix::fs::MetadataExt as _;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use quanta_index_catalog::{CatalogSnapshotReceiptV1, CatalogSnapshotSessionV1, SqliteCatalog};
use quanta_index_core::{CoreError, RequestBudgetV1};
use quanta_index_repomap::RepoMapGenerationStore;
use quanta_index_search_plane::ActivationCatalog;
use quanta_index_searchd::app::config::{LegacyPublicationPhysicalPolicy, SearchdConfig};
use quanta_index_searchd::app::runtime::StateRootLease;
use quanta_index_searchd::app::state_format::{
    PreparedStateRootV1, STATE_BACKUP_CATALOG_FILE_NAME, STATE_CATALOG_DIRECTORY,
    StateMigrationFaultPort, StateObjectEntryV1, StateRootManifestV1, prepare_staging_root_v1,
};
use quanta_index_searchd::app::state_migration::{
    CatalogFreezeV1, CatalogSnapshotV1, OfflineSourceCustodyV1, OfflineSourceSessionV1,
    OfflineTargetFactoryV1, OfflineTargetSessionV1, StateRootDeepOpenReceiptV1,
};
use rusqlite::ffi::quanta_capacity_vfs::close_owned_file;
use rustix::fs::{Mode, OFlags, open};

use super::{OFFLINE_CATALOG_BUSY_BUDGET, map_receipt, verify_staged_lexical_generation};
use crate::legacy_physical_assembly::{LegacyPhysicalAssembly, LegacyPhysicalAuthority};
use crate::offline_target_file_io::OfflineTargetFileIo;

fn contract(message: impl Into<String>) -> CoreError {
    CoreError::InvalidContract(format!("offline destination: {}", message.into()))
}

fn with_secondary(mut primary: CoreError, context: &str, secondary: &CoreError) -> CoreError {
    let message = match &mut primary {
        CoreError::InvalidContract(message)
        | CoreError::NotReady(message)
        | CoreError::NotImplemented(message)
        | CoreError::NotFound(message)
        | CoreError::Storage(message)
        | CoreError::StorageIo { message, .. }
        | CoreError::Typed { message, .. } => message,
    };
    let _written = write!(message, "; {context}: {secondary}");
    primary
}

struct OfflineTargetFactory {
    policy: LegacyPublicationPhysicalPolicy,
    source_body_max_bytes: u64,
}

/// Resolve the existing deployment policy before any source/target mutation.
pub fn offline_target_factory_v1(
    config: &SearchdConfig,
) -> Result<impl OfflineTargetFactoryV1, CoreError> {
    let policy = config
        .legacy_publication_physical_policy()
        .ok_or_else(|| contract("offline target requires an explicit physical policy"))?
        .validate()
        .map_err(|error| contract(format!("invalid physical policy: {error}")))?;
    let source_body_max_bytes = config.source_publication_max_bytes();
    if source_body_max_bytes == 0 || source_body_max_bytes > policy.inventory.bytes {
        return Err(contract("source body maximum exceeds the root capacity"));
    }
    Ok(OfflineTargetFactory {
        policy,
        source_body_max_bytes,
    })
}

impl OfflineTargetFactoryV1 for OfflineTargetFactory {
    fn begin_target(
        &self,
        source: &OfflineSourceSessionV1,
        destination: &Path,
        budget: &RequestBudgetV1,
    ) -> Result<Box<dyn OfflineTargetSessionV1>, CoreError> {
        budget.checkpoint("offline target before source pin")?;
        let mut source_pin = SourcePin {
            custody: Some(source.custody().clone()),
            file: Some(File::from(
                open(
                    source.canonical_root(),
                    OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
                    Mode::empty(),
                )
                .map_err(|error| {
                    CoreError::storage_io("open original offline source directory", &error.into())
                })?,
            )),
        };
        let metadata = source_pin
            .file
            .as_ref()
            .ok_or_else(|| contract("source pin absent"))?
            .metadata()
            .map_err(|error| {
                CoreError::storage_io("stat original offline source directory", &error)
            })?;
        if metadata.dev() != source.before().root_device
            || metadata.ino() != source.before().root_inode
        {
            return Err(CoreError::Typed {
                code: quanta_index_contract::SearchPlaneErrorCodeV2::StateRootInsecure,
                message: "offline source root changed since its custody was frozen".into(),
            });
        }
        let prepared = prepare_staging_root_v1(destination, budget)?;
        let staging = prepared.path().to_path_buf();
        let mut target = OfflineTarget {
            staging,
            prepared: Some(prepared),
            source_path: source.canonical_root().to_path_buf(),
            source_custody: source_pin.custody.take(),
            source_root: source_pin.file.take(),
            authority: None,
            cutover_lease: None,
            files: None,
            domains: None,
            snapshot: None,
            catalog: None,
            budget: budget.clone(),
            retired: false,
            cutover_failed: false,
        };
        let lease = StateRootLease::acquire(&target.staging)?;
        // All adapter paths must use the lease's original canonical namespace
        // (for example /private/tmp rather than its /tmp alias on macOS).
        target.staging = lease.state_root_identity_v1().to_path_buf();
        target.authority = Some(LegacyPhysicalAuthority::new(
            lease,
            self.policy,
            self.source_body_max_bytes,
            budget,
            &[],
        )?);
        let authority = target.authority()?;
        target.files = Some(OfflineTargetFileIo::new(
            authority.binder(),
            authority.deletions(),
            authority.state_root_lease().try_clone_root_directory_v1()?,
            budget.clone(),
        )?);
        Ok(Box::new(target))
    }
}

// Keep original source custody even when preparation fails before a target
// authority exists. A native close uncertainty cannot become clean teardown.
struct SourcePin {
    custody: Option<OfflineSourceCustodyV1>,
    file: Option<File>,
}
impl Drop for SourcePin {
    fn drop(&mut self) {
        if let Some(file) = self.file.take()
            && close_owned_file(file).is_err()
        {
            std::mem::forget(self.custody.take());
        }
    }
}

struct OfflineTarget {
    staging: PathBuf,
    prepared: Option<PreparedStateRootV1>,
    source_path: PathBuf,
    source_custody: Option<OfflineSourceCustodyV1>,
    source_root: Option<File>,
    authority: Option<LegacyPhysicalAuthority>,
    cutover_lease: Option<StateRootLease>,
    files: Option<OfflineTargetFileIo>,
    domains: Option<LegacyPhysicalAssembly>,
    snapshot: Option<CatalogSnapshotSessionV1>,
    catalog: Option<Arc<SqliteCatalog>>,
    budget: RequestBudgetV1,
    retired: bool,
    cutover_failed: bool,
}

impl OfflineTarget {
    fn authority(&self) -> Result<&LegacyPhysicalAuthority, CoreError> {
        self.authority
            .as_ref()
            .ok_or_else(|| contract("target authority absent"))
    }
    fn files(&self) -> Result<&OfflineTargetFileIo, CoreError> {
        self.files
            .as_ref()
            .ok_or_else(|| contract("target file authority retired"))
    }
    fn catalog_relative() -> PathBuf {
        Path::new(STATE_CATALOG_DIRECTORY).join(STATE_BACKUP_CATALOG_FILE_NAME)
    }
    fn ensure_snapshot_owner(&mut self) -> Result<(), CoreError> {
        if self.snapshot.is_none() {
            let io = self.authority()?.catalog_io()?;
            self.snapshot = Some(CatalogSnapshotSessionV1::new(
                self.staging.join(Self::catalog_relative()),
                io,
            ));
        }
        Ok(())
    }
    fn settle_snapshot(
        &self,
        mut receipt: CatalogSnapshotReceiptV1,
    ) -> Result<CatalogSnapshotV1, CoreError> {
        // sqlite3_close may queue physical FD retirement. Drain its actual
        // callbacks before a later boot scan requires no live native pins;
        // keep the same reusable Catalog scope and original held funding.
        self.settle_catalog_retirements()?;
        let width = self.files()?.sync_file(&Self::catalog_relative())?;
        if receipt.byte_size != width {
            return Err(contract(format!(
                "snapshot logical width {} differs from actual original inode width {width}",
                receipt.byte_size
            )));
        }
        receipt.byte_size = width;
        Ok(map_receipt(&receipt))
    }
    fn settle_catalog_retirements(&self) -> Result<(), CoreError> {
        let authority = self.authority()?;
        authority.catalog_io()?.drain_retirements()?;
        // SQLite can open an empty WAL while normalizing a copied header.
        // It contains no frames and never receives xSync. Dispose only that
        // original zero-byte birth after actual native close, never a resize
        // or populated WAL, through the existing checked absence protocol.
        authority
            .binder()
            .discard_retired_empty_native_create(&self.staging.join(format!(
                "{STATE_CATALOG_DIRECTORY}/{STATE_BACKUP_CATALOG_FILE_NAME}-wal"
            )))
    }
    fn close_catalog(&mut self) -> Result<(), CoreError> {
        if let Some(catalog) = self.catalog.as_mut() {
            Arc::get_mut(catalog)
                .ok_or_else(|| CoreError::NotReady("offline Catalog still has consumers".into()))?
                .close_checked()?;
            self.settle_catalog_retirements()?;
        }
        drop(self.catalog.take());
        Ok(())
    }
    fn retire(&mut self) -> Result<(), CoreError> {
        if self.retired {
            return Ok(());
        }
        if let Some(snapshot) = self.snapshot.as_mut() {
            snapshot.close_checked()?;
            self.settle_catalog_retirements()?;
        }
        drop(self.snapshot.take());
        self.close_catalog()?;
        drop(self.domains.take());
        if let Some(files) = self.files.as_ref() {
            files.reconcile_pending()?;
        }
        drop(self.files.take());
        if let Some(source) = self.source_root.take()
            && let Err(error) = close_owned_file(source)
        {
            if let Some(authority) = self.authority.as_ref() {
                authority.binder().mark_close_unwitnessed();
            }
            return Err(CoreError::storage_io(
                "close original offline source root FD",
                &error,
            ));
        }
        if let Some(authority) = self.authority.as_mut() {
            authority.retire_checked()?;
        }
        // Mutable IO is retired, but the same root lock remains held through
        // the namespace switch and parent sync, so another cooperative owner
        // cannot change the already verified image in that interval.
        if let Some(authority) = self.authority.as_ref() {
            self.cutover_lease = Some(authority.state_root_lease().clone());
        }
        drop(self.authority.take());
        self.retired = true;
        Ok(())
    }
    fn deep_open_work(&mut self) -> Result<StateRootDeepOpenReceiptV1, CoreError> {
        self.budget.checkpoint("offline domain assembly")?;
        if self.catalog.is_some() {
            return Err(CoreError::NotReady(
                "original offline Catalog must retire before another deep open".into(),
            ));
        }
        if self.domains.is_none() {
            self.domains = Some(self.authority()?.build_domains()?);
        }
        let domains = self
            .domains
            .as_ref()
            .ok_or_else(|| contract("target domains absent"))?;
        self.catalog = Some(Arc::new(SqliteCatalog::open_with_io_admission(
            &self.staging,
            OFFLINE_CATALOG_BUSY_BUDGET,
            domains.catalog_io.clone(),
        )?));
        let catalog = self
            .catalog
            .as_ref()
            .ok_or_else(|| contract("target Catalog absent"))?;
        let candidates = catalog.repomap_candidate_rows()?;
        let repomap_candidates = u64::try_from(candidates.len())
            .map_err(|error| contract(format!("candidate count: {error}")))?;
        self.budget.checkpoint("offline staged RepoMap open")?;
        let repomap = RepoMapGenerationStore::open_with_file_admission(
            self.staging.join("repo-map"),
            catalog.clone(),
            domains.repomap.clone(),
        )?;
        self.budget.checkpoint("offline staged index inventory")?;
        let lexical = quanta_index_lexical::inventory_sealed_generations(
            &self.staging.join("indexes/lexical"),
        )?;
        let lexical_adapter = quanta_index_lexical::LexicalAdapter::with_state_root(
            self.staging.join("indexes/lexical"),
        )
        .with_object_lifetime(domains.publication.object_lifetime());
        for sealed in &lexical.sealed {
            verify_staged_lexical_generation(&lexical_adapter, &sealed.identity, &self.budget)?;
        }
        let semantic = quanta_index_semantic::inventory_persisted_generations(
            &quanta_index_semantic::semantic_state_root(&self.staging),
        )?;
        let sealed_generations = u64::try_from(
            lexical
                .sealed
                .len()
                .checked_add(semantic.sealed.len())
                .ok_or_else(|| contract("sealed generation count overflow"))?,
        )
        .map_err(|error| contract(format!("sealed count: {error}")))?;
        drop(repomap);
        Ok(StateRootDeepOpenReceiptV1 {
            sealed_generations,
            repomap_candidates,
            catalog_rows: repomap_candidates,
        })
    }
}

impl OfflineTargetSessionV1 for OfflineTarget {
    fn staging_root(&self) -> &Path {
        &self.staging
    }
    fn ensure_directory(&mut self, relative: &Path) -> Result<(), CoreError> {
        self.files()?.ensure_relative_directories(relative)
    }
    fn copy_object(&mut self, object: &StateObjectEntryV1) -> Result<(), CoreError> {
        self.files()?.copy_object(
            self.source_root
                .as_ref()
                .ok_or_else(|| contract("original source FD absent"))?,
            Path::new(&object.relative_path),
            object.byte_size,
            &object.digest_hex,
        )
    }
    fn snapshot_catalog(&mut self) -> Result<CatalogFreezeV1, CoreError> {
        self.budget.checkpoint("offline Catalog snapshot")?;
        self.files()?.create_empty_file(&Self::catalog_relative())?;
        self.ensure_snapshot_owner()?;
        let (live, snapshot) = self
            .snapshot
            .as_mut()
            .ok_or_else(|| contract("snapshot owner absent"))?
            .snapshot_with_source_receipt(
                &self.source_path.join(Self::catalog_relative()),
                OFFLINE_CATALOG_BUSY_BUDGET,
                &self.budget,
            )?;
        Ok(CatalogFreezeV1 {
            live: map_receipt(&live),
            snapshot: self.settle_snapshot(snapshot)?,
        })
    }
    fn verify_catalog(&mut self) -> Result<CatalogSnapshotV1, CoreError> {
        self.ensure_snapshot_owner()?;
        let receipt = self
            .snapshot
            .as_mut()
            .ok_or_else(|| contract("snapshot owner absent"))?
            .verify(&self.budget)?;
        self.settle_snapshot(receipt)
    }
    fn rotate_incarnation(&mut self) -> Result<(), CoreError> {
        self.files()?
            .ensure_relative_directories(Path::new("activations"))?;
        let relative = Path::new("activations/.activation-root-incarnation-v1");
        let previous = self
            .files()?
            .read_bytes(relative, 16)?
            .map(|bytes| {
                <[u8; 16]>::try_from(bytes).map_err(|bytes| {
                    contract(format!(
                        "activation incarnation width {} differs from 16",
                        bytes.len()
                    ))
                })
            })
            .transpose()?;
        self.budget
            .checkpoint("offline activation incarnation plan")?;
        self.authority()?.binder().validate_current_state_root()?;
        let has_heads = fs::read_dir(self.staging.join("activations"))
            .map_err(|error| CoreError::storage_io("list admitted activation names", &error))?
            .try_fold(false, |found, entry| {
                self.budget
                    .checkpoint("offline activation incarnation listing")?;
                let entry = entry.map_err(|error| {
                    CoreError::storage_io("read admitted activation name", &error)
                })?;
                Ok::<bool, CoreError>(
                    found
                        || entry
                            .file_name()
                            .to_str()
                            .is_some_and(|name| name.ends_with("--corpus.json")),
                )
            })?;
        self.authority()?.binder().validate_current_state_root()?;
        let next = ActivationCatalog::restore_incarnation_bytes_v1(previous, has_heads)?;
        self.files()?.write_bytes(relative, &next, None)
    }
    fn deep_open(&mut self) -> Result<StateRootDeepOpenReceiptV1, CoreError> {
        let work = self.deep_open_work();
        let close = self.close_catalog();
        match (work, close) {
            (Ok(receipt), Ok(())) => Ok(receipt),
            (Err(primary), Ok(())) | (Ok(_), Err(primary)) => Err(primary),
            (Err(primary), Err(close)) => {
                Err(with_secondary(primary, "original Catalog close", &close))
            }
        }
    }
    fn write_manifest(
        &mut self,
        name: &str,
        manifest: &StateRootManifestV1,
        fault: &dyn StateMigrationFaultPort,
    ) -> Result<(), CoreError> {
        self.files()?
            .write_bytes(Path::new(name), manifest.encode().as_bytes(), Some(fault))
    }
    fn publish(&mut self) -> Result<(), CoreError> {
        self.budget
            .checkpoint("offline target before checked cutover")?;
        self.retire()?;
        let outcome = self
            .prepared
            .as_ref()
            .ok_or_else(|| contract("original namespace owner absent"))?
            .publish_noreplace();
        self.cutover_failed = outcome.is_err();
        outcome
    }
}

impl Drop for OfflineTarget {
    fn drop(&mut self) {
        if self.retire().is_err() || self.cutover_failed {
            // Failed custody is never released as a clean command outcome.
            // Reuse the process-lifetime quarantine convention of runtime
            // retirement; this retains both original source and target leases.
            std::mem::forget((
                self.snapshot.take(),
                self.catalog.take(),
                self.domains.take(),
                self.files.take(),
                self.authority.take(),
                self.cutover_lease.take(),
                self.prepared.take(),
                self.source_root.take(),
                self.source_custody.take(),
            ));
        }
    }
}
