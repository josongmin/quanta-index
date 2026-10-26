use std::collections::BTreeSet;
use std::error::Error;
use std::fs;
use std::os::unix::fs::{MetadataExt as _, PermissionsExt as _};
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use quanta_index_catalog::{CatalogIoAdmission, SqliteCatalog};
use quanta_index_core::{CoreError, RequestBudgetV1};
use quanta_index_searchd::app::config::TEST_STATE_ROOT_PHYSICAL_POLICY;
use quanta_index_searchd::app::runtime::{RuntimeRetirementPort as _, StateRootLease};

use super::LegacyRuntimeRetirement;
use crate::legacy_physical_assembly::LegacyPhysicalAssembly;

type TestResult = Result<(), Box<dyn Error>>;

fn regular_file_inventory(root: &Path) -> Result<(u64, u64), Box<dyn Error>> {
    let mut pending = vec![root.to_path_buf()];
    let mut bytes = 0_u64;
    let mut files = 0_u64;
    let mut seen = BTreeSet::new();
    while let Some(directory) = pending.pop() {
        for entry in fs::read_dir(directory)? {
            let entry = entry?;
            let metadata = entry.path().symlink_metadata()?;
            if metadata.is_dir() {
                pending.push(entry.path());
            } else if metadata.is_file() {
                if !seen.insert((metadata.dev(), metadata.ino())) {
                    continue;
                }
                bytes = bytes
                    .checked_add(metadata.len())
                    .ok_or("inventory byte overflow")?;
                files = files.checked_add(1).ok_or("inventory file overflow")?;
            } else {
                return Err("unexpected non-regular entry in private state root".into());
            }
        }
    }
    Ok((bytes, files))
}

fn retire_one_legacy_root(directory: &tempfile::TempDir) -> TestResult {
    let requested = directory.path().join("state");
    fs::create_dir(&requested)?;
    fs::set_permissions(&requested, fs::Permissions::from_mode(0o700))?;
    let lease = StateRootLease::acquire(&requested)?;
    let root = lease.state_root_identity_v1().to_path_buf();
    let physical = LegacyPhysicalAssembly::new(
        lease,
        TEST_STATE_ROOT_PHYSICAL_POLICY,
        4096,
        &RequestBudgetV1::unbounded(),
        &[],
    )?;
    let LegacyPhysicalAssembly {
        state_root_lease,
        publication,
        catalog_io,
        catalog_funding,
        durable_control,
        source_upload,
        repomap,
        history,
        semantic,
        semantic_maintenance,
    } = physical;
    drop((
        durable_control,
        source_upload,
        repomap,
        history,
        semantic,
        semantic_maintenance,
    ));
    let deletions = publication.file_deletions();
    let binder = deletions.binder();
    if catalog_io.vfs_name() != "quanta-publication-capacity" {
        return Err("Legacy assembly registered an unexpected process VFS name".into());
    }
    let admission: Arc<dyn CatalogIoAdmission> = catalog_io.clone();
    let catalog = Arc::new(SqliteCatalog::open_with_io_admission(
        &root,
        Duration::from_secs(1),
        admission,
    )?);
    let catalog_file = catalog.path().to_path_buf();
    let (_next, _exhausted) = catalog.sequence_allocator()?;
    let foreign_catalog = Arc::clone(&catalog);
    let retirement = LegacyRuntimeRetirement::new(catalog, catalog_io.clone(), deletions.clone());
    let before = binder.capacity_snapshot()?;
    let refused = retirement.retire();
    if !matches!(refused, Err(CoreError::NotReady(_)))
        || binder.capacity_snapshot()? != before
        || StateRootLease::acquire_existing(&root).is_ok()
    {
        return Err("foreign Catalog owner was retired or original capacity/lease changed".into());
    }
    drop(foreign_catalog);
    drop(catalog_io);
    retirement.retire()?;
    retirement.retire()?;
    let settled = binder.capacity_snapshot()?;
    let (disk_bytes, disk_files) = regular_file_inventory(&root)?;
    let catalog_bytes = fs::metadata(&catalog_file)?.len();
    if settled.used_bytes != disk_bytes
        || settled.used_files != disk_files
        || settled.active_allowances != 0
        || settled.active_tickets != 0
        || settled.uncertain_pending != 0
        || settled.poisoned
        || !binder.is_quiescent()?
        || catalog_bytes == 0
    {
        return Err("checked retirement lost Catalog disk/ledger parity or pending custody".into());
    }
    drop(retirement);
    drop(deletions);
    drop(publication);
    drop(catalog_funding);
    drop(binder);
    drop(state_root_lease);
    let reacquired = StateRootLease::acquire_existing(&root)?;
    drop(reacquired);
    Ok(())
}

#[test]
fn production_retirement_retains_original_owner_until_catalog_consumers_close() -> TestResult {
    let directory = tempfile::tempdir()?;
    retire_one_legacy_root(&directory)
}

#[test]
fn two_legacy_roots_reuse_one_process_vfs_after_checked_retirement() -> TestResult {
    let first = tempfile::tempdir()?;
    let second = tempfile::tempdir()?;
    if first.path() == second.path() {
        return Err("two Legacy roots share a filesystem path".into());
    }
    retire_one_legacy_root(&first)?;
    retire_one_legacy_root(&second)?;
    Ok(())
}
