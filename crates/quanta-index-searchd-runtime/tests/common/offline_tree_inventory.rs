//! Source custody and name inventory for the offline-state regressions.

use std::fs;
use std::path::Path;

use quanta_index_searchd::app::runtime::StateRootLease;
use quanta_index_searchd::app::state_format::STATE_ROOT_MANIFEST_FILE_NAME;
use quanta_index_searchd::app::state_migration::{OfflineSourceSessionV1, SourceFreezeReceiptV1};

/// The frozen inventory of a current root under its daemon lease.
pub(super) fn freeze_current(
    root: &Path,
) -> Result<SourceFreezeReceiptV1, Box<dyn std::error::Error>> {
    let session = OfflineSourceSessionV1::open_current(StateRootLease::acquire(root)?)?;
    Ok(session.before().clone())
}

/// Migration authority files belong in staging or destination. Ordinary
/// track manifests and the existing daemon lock remain valid source inputs.
pub(super) fn assert_no_source_markers(root: &Path) -> Result<(), Box<dyn std::error::Error>> {
    let mut stack = vec![root.to_path_buf()];
    while let Some(directory) = stack.pop() {
        for entry in fs::read_dir(&directory)? {
            let entry = entry?;
            let name = entry.file_name();
            let name = name.to_string_lossy();
            let path = entry.path();
            if name == "MIGRATED"
                || name.starts_with("MIGRATED.")
                || name.starts_with(".MIGRATED.")
                || name == STATE_ROOT_MANIFEST_FILE_NAME
                || name == "state-migration-receipt-v1.txt"
            {
                return Err(format!(
                    "source root {} holds the authority file {}",
                    root.display(),
                    path.display()
                )
                .into());
            }
            if entry.file_type()?.is_dir() {
                stack.push(path);
            }
        }
    }
    Ok(())
}

/// Compare source names while allowing the Catalog to manage its own files.
pub(crate) fn tree_names_outside_catalog(
    root: &Path,
) -> Result<Vec<String>, Box<dyn std::error::Error>> {
    let mut names = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(directory) = stack.pop() {
        for entry in fs::read_dir(&directory)? {
            let entry = entry?;
            let path = entry.path();
            let relative = path
                .strip_prefix(root)?
                .to_string_lossy()
                .replace('\\', "/");
            if relative == "catalog" || relative.starts_with("catalog/") {
                continue;
            }
            names.push(relative);
            if entry.file_type()?.is_dir() {
                stack.push(path);
            }
        }
    }
    names.sort();
    Ok(names)
}
