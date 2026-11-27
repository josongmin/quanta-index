//! Hold an existing Catalog directory under the publication root while a
//! protected SQLite reader owns its main and optional WAL descriptors.

use super::*;
use quanta_index_catalog::CatalogReadOnlyIoAdmission;
use quanta_index_core::{
    REQUEST_CANCELLED_CODE, REQUEST_DEADLINE_EXCEEDED_CODE, RequestBudgetV1, index::IndexError,
};
use rustix::fs::{AtFlags, FileType, statat};

struct ProtectedCatalogSlots<'a> {
    common: &'a PublicationFileCapacity,
    count: usize,
    retired: bool,
}

impl<'a> ProtectedCatalogSlots<'a> {
    fn acquire(common: &'a PublicationFileCapacity, count: usize) -> Result<Self, CoreError> {
        let mut state = lock(&common.shared.state)?;
        require_publication_open(&state)?;
        let used = state
            .total_pins
            .checked_add(state.total_opening_readers)
            .and_then(|used| used.checked_add(state.total_metadata_readers))
            .and_then(|used| used.checked_add(count))
            .ok_or_else(|| capacity("protected Catalog reader handle count overflow"))?;
        if used > common.shared.max_open_handles {
            return Err(capacity("protected Catalog reader handle ceiling exceeded"));
        }
        state.total_metadata_readers = state
            .total_metadata_readers
            .checked_add(count)
            .ok_or_else(|| capacity("protected Catalog reader handle count overflow"))?;
        drop(state);
        Ok(Self {
            common,
            count,
            retired: false,
        })
    }

    fn add_wal(&mut self) -> Result<(), CoreError> {
        if self.count != 2 {
            return Err(contract("protected Catalog WAL slot already reserved"));
        }
        let mut state = lock(&self.common.shared.state)?;
        require_publication_open(&state)?;
        let used = state
            .total_pins
            .checked_add(state.total_opening_readers)
            .and_then(|used| used.checked_add(state.total_metadata_readers))
            .and_then(|used| used.checked_add(1))
            .ok_or_else(|| capacity("protected Catalog WAL handle count overflow"))?;
        if used > self.common.shared.max_open_handles {
            return Err(capacity("protected Catalog WAL handle ceiling exceeded"));
        }
        state.total_metadata_readers = state
            .total_metadata_readers
            .checked_add(1)
            .ok_or_else(|| capacity("protected Catalog WAL handle count overflow"))?;
        self.count = 3;
        Ok(())
    }

    fn retire(mut self) -> Result<(), CoreError> {
        let mut state = lock(&self.common.shared.state)?;
        if state.close_unwitnessed {
            return Err(storage(
                "protected Catalog reader",
                "owned close was ambiguous",
            ));
        }
        state.total_metadata_readers = state
            .total_metadata_readers
            .checked_sub(self.count)
            .ok_or_else(|| storage("protected Catalog reader", "handle count underflow"))?;
        self.retired = true;
        Ok(())
    }
}

impl Drop for ProtectedCatalogSlots<'_> {
    fn drop(&mut self) {
        if !self.retired {
            self.common.mark_close_unwitnessed();
        }
    }
}

fn index_error(error: CoreError) -> IndexError {
    match error {
        CoreError::Typed { code, .. } if code == REQUEST_CANCELLED_CODE => IndexError::Cancelled,
        CoreError::Typed { code, .. }
            if code == REQUEST_DEADLINE_EXCEEDED_CODE
                || code == SearchPlaneErrorCodeV2::IngestResourceBudgetExceeded =>
        {
            IndexError::ResourceLimited
        }
        other => IndexError::Storage(format!("protected Catalog directory: {other}")),
    }
}

fn wal_present(directory: &File, name: &Path) -> Result<bool, CoreError> {
    match statat(directory, name, AtFlags::SYMLINK_NOFOLLOW) {
        Ok(stat) => {
            if FileType::from_raw_mode(stat.st_mode) != FileType::RegularFile {
                return Err(contract("protected Catalog WAL is not a regular file"));
            }
            Ok(true)
        }
        Err(rustix::io::Errno::NOENT) => Ok(false),
        Err(error) => Err(storage_io("stat protected Catalog WAL", error)),
    }
}

fn same_directory(left: &std::fs::Metadata, right: &std::fs::Metadata) -> bool {
    left.is_dir()
        && right.is_dir()
        && left.dev() == right.dev()
        && left.ino() == right.ino()
        && left.nlink() == right.nlink()
        && left.uid() == right.uid()
        && left.mode() == right.mode()
        && left.mtime() == right.mtime()
        && left.mtime_nsec() == right.mtime_nsec()
        && left.ctime() == right.ctime()
        && left.ctime_nsec() == right.ctime_nsec()
}

impl CatalogReadOnlyIoAdmission for PublicationFileCapacity {
    fn with_protected_catalog_directory_v1(
        &self,
        directory: &Path,
        request: &RequestBudgetV1,
        needs_wal: bool,
        visit: &mut dyn FnMut(&File),
    ) -> Result<bool, IndexError> {
        PublicationFileCapacity::with_protected_catalog_directory_v1(
            self, directory, request, needs_wal, visit,
        )?;
        Ok(true)
    }

    fn poison_protected_catalog_close_v1(&self) {
        self.mark_close_unwitnessed();
    }
}

impl PublicationFileCapacity {
    /// The callback cannot outlive this anchored directory, the root lease,
    /// or the slots reserved for SQLite's main and WAL descriptors.
    pub(crate) fn with_protected_catalog_directory_v1(
        &self,
        directory: &Path,
        request: &RequestBudgetV1,
        needs_wal: bool,
        visit: &mut dyn FnMut(&File),
    ) -> Result<(), IndexError> {
        self.with_catalog_directory_scope_v1(
            directory,
            request,
            Some(needs_wal),
            &mut |held, _observed_wal| visit(held),
        )
    }

    /// Discover WAL presence only after opening the anchored directory. The
    /// third descriptor slot is reserved before the visitor can open the WAL.
    pub(crate) fn with_observed_catalog_directory_v1(
        &self,
        directory: &Path,
        request: &RequestBudgetV1,
        visit: &mut dyn FnMut(&File, bool),
    ) -> Result<(), IndexError> {
        self.with_catalog_directory_scope_v1(directory, request, None, visit)
    }

    fn with_catalog_directory_scope_v1(
        &self,
        directory: &Path,
        request: &RequestBudgetV1,
        expected_wal: Option<bool>,
        visit: &mut dyn FnMut(&File, bool),
    ) -> Result<(), IndexError> {
        request
            .checkpoint("open protected Catalog directory")
            .map_err(index_error)?;
        if directory != quanta_index_catalog::catalog_dir(self.state_root()) {
            return Err(IndexError::BackendViolation(
                "protected Catalog directory belongs to another root".into(),
            ));
        }
        let catalog_name = directory.file_name().ok_or_else(|| {
            IndexError::BackendViolation("protected Catalog directory has no basename".into())
        })?;
        let wal_name = format!("{}-wal", quanta_index_catalog::CATALOG_FILE_NAME);
        let wal_name = Path::new(&wal_name);
        let initial_slots = if expected_wal == Some(true) { 3 } else { 2 };
        let mut slots = ProtectedCatalogSlots::acquire(self, initial_slots).map_err(index_error)?;
        let mut post_uncertain = false;
        let result: Result<(), CoreError> =
            self.with_existing_list_scope_v1(directory, true, |held| {
                let before = held
                    .metadata()
                    .map_err(|error| storage_io("stat held protected Catalog directory", error))?;
                let observed_wal = wal_present(held, wal_name)?;
                if let Some(expected_wal) = expected_wal {
                    if observed_wal != expected_wal {
                        return Err(contract(
                            "protected Catalog WAL presence changed before open",
                        ));
                    }
                } else if observed_wal {
                    slots.add_wal()?;
                }
                request.checkpoint("inspect protected Catalog directory")?;
                visit(held, observed_wal);

                // The FD alone could still name a detached directory after a root
                // rename. Check both live names before the result can be promoted.
                let checked = (|| -> Result<(), CoreError> {
                    self.shared.root.require_current_v1()?;
                    let after = held.metadata().map_err(|error| {
                        storage_io("restat held protected Catalog directory", error)
                    })?;
                    let linked = statat(
                        &self.shared.root,
                        Path::new(catalog_name),
                        AtFlags::SYMLINK_NOFOLLOW,
                    )
                    .map_err(|error| {
                        storage_io("restat linked protected Catalog directory", error)
                    })?;
                    if !same_directory(&before, &after)
                        || FileType::from_raw_mode(linked.st_mode) != FileType::Directory
                        || checked_device_id(linked.st_dev)
                            .map_err(|error| storage("protected Catalog device", error))?
                            != before.dev()
                        || linked.st_ino != before.ino()
                        || wal_present(held, wal_name)? != observed_wal
                    {
                        return Err(contract("protected Catalog directory or WAL link changed"));
                    }
                    Ok(())
                })();
                if checked.is_err() {
                    post_uncertain = true;
                }
                checked
            });
        if post_uncertain {
            self.mark_close_unwitnessed();
        }
        let retired = slots.retire().map_err(index_error);
        let result = result.map_err(|error| match (expected_wal, error) {
            (None, CoreError::NotFound(_)) => IndexError::SnapshotUnavailable,
            (_, error) => index_error(error),
        });
        request
            .checkpoint("close protected Catalog directory")
            .map_err(index_error)?;
        result?;
        retired
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::publication_inventory::{PublicationBootInventoryLimits, seed_publication_capacity};
    use quanta_index_searchd::app::runtime::StateRootLease;
    use std::{os::unix::fs::PermissionsExt as _, time::Duration};

    type TestResult = Result<(), Box<dyn std::error::Error>>;

    fn fixture(
        needs_wal: bool,
        handles: usize,
        expected_quiescent: bool,
        run: impl FnOnce(&Path, &PublicationFileCapacity, &RequestBudgetV1) -> TestResult,
    ) -> TestResult {
        let temp = tempfile::tempdir()?;
        let lease = StateRootLease::acquire(&temp.path().join("state"))?;
        let root = lease.state_root_identity_v1().to_path_buf();
        let catalog = quanta_index_catalog::catalog_dir(&root);
        std::fs::create_dir(&catalog)?;
        std::fs::set_permissions(&catalog, std::fs::Permissions::from_mode(0o700))?;
        let main = catalog.join(quanta_index_catalog::CATALOG_FILE_NAME);
        std::fs::write(&main, b"existing Catalog input")?;
        std::fs::set_permissions(&main, std::fs::Permissions::from_mode(0o600))?;
        if needs_wal {
            let wal = catalog.join(format!("{}-wal", quanta_index_catalog::CATALOG_FILE_NAME));
            std::fs::write(&wal, b"committed WAL input")?;
            std::fs::set_permissions(&wal, std::fs::Permissions::from_mode(0o600))?;
        }
        let request = RequestBudgetV1::for_duration(Duration::from_secs(30));
        let capacity = seed_publication_capacity(
            &lease,
            &root,
            PublicationBootInventoryLimits {
                max_bytes: 1024 * 1024,
                max_files: 16,
                max_pending: 16,
                max_entries: 32,
                max_depth: 8,
                max_pending_frames: 16,
            },
            &[],
            &request,
        )?;
        let common = PublicationFileCapacity::new(lease, capacity, handles, 64, 256)?;
        run(&catalog, &common, &request)?;
        if common.is_quiescent()? != expected_quiescent {
            return Err("protected Catalog reader quiescence differed from expectation".into());
        }
        Ok(())
    }

    #[test]
    fn anchored_catalog_reader_reserves_exact_main_and_wal_slots() -> TestResult {
        for needs_wal in [false, true] {
            let expected = if needs_wal { 3 } else { 2 };
            fixture(needs_wal, expected, true, |catalog, common, request| {
                let mut visited = 0;
                common.with_protected_catalog_directory_v1(
                    catalog,
                    request,
                    needs_wal,
                    &mut |held| {
                        visited += 1;
                        assert!(held.metadata().is_ok_and(|metadata| metadata.is_dir()));
                        let state = lock(&common.shared.state).expect("tracking mutex");
                        assert_eq!(state.total_metadata_readers, expected);
                    },
                )?;
                if visited != 1 {
                    return Err("anchored Catalog visitor count differed".into());
                }
                Ok(())
            })?;
        }
        Ok(())
    }

    #[test]
    fn anchored_catalog_reader_discovers_wal_before_visiting() -> TestResult {
        for needs_wal in [false, true] {
            let expected = if needs_wal { 3 } else { 2 };
            fixture(needs_wal, expected, true, |catalog, common, request| {
                let mut visited = false;
                common.with_observed_catalog_directory_v1(
                    catalog,
                    request,
                    &mut |_held, observed_wal| {
                        visited = true;
                        assert_eq!(observed_wal, needs_wal);
                        let state = lock(&common.shared.state).expect("tracking mutex");
                        assert_eq!(state.total_metadata_readers, expected);
                    },
                )?;
                if !visited {
                    return Err("observed Catalog visitor was not called".into());
                }
                Ok(())
            })?;
        }
        fixture(true, 2, true, |catalog, common, request| {
            let mut visited = false;
            let result = common.with_observed_catalog_directory_v1(
                catalog,
                request,
                &mut |_held, _observed_wal| visited = true,
            );
            if !matches!(result, Err(IndexError::ResourceLimited)) || visited {
                return Err("unfunded observed WAL reached Catalog visitor".into());
            }
            Ok(())
        })
    }

    #[test]
    fn anchored_catalog_reader_refuses_new_wal_and_unfunded_fd() -> TestResult {
        fixture(false, 2, true, |catalog, common, request| {
            let wal = catalog.join(format!("{}-wal", quanta_index_catalog::CATALOG_FILE_NAME));
            std::fs::write(&wal, b"new WAL")?;
            std::fs::set_permissions(&wal, std::fs::Permissions::from_mode(0o600))?;
            let mut visited = false;
            let result =
                common.with_protected_catalog_directory_v1(catalog, request, false, &mut |_held| {
                    visited = true
                });
            if result.is_ok() || visited {
                return Err("WAL appearance was accepted before anchored Catalog open".into());
            }
            Ok(())
        })?;
        fixture(true, 2, true, |catalog, common, request| {
            let mut visited = false;
            let result =
                common.with_protected_catalog_directory_v1(catalog, request, true, &mut |_held| {
                    visited = true
                });
            if !matches!(result, Err(IndexError::ResourceLimited)) || visited {
                return Err("unfunded WAL descriptor reached anchored Catalog visitor".into());
            }
            Ok(())
        })
    }

    #[test]
    fn anchored_catalog_reader_poison_on_directory_replacement() -> TestResult {
        fixture(false, 2, false, |catalog, common, request| {
            let moved = catalog.with_file_name("catalog-moved");
            let mut mutation = Ok(());
            let mut visited = false;
            let result =
                common.with_protected_catalog_directory_v1(catalog, request, false, &mut |_held| {
                    visited = true;
                    mutation = (|| -> std::io::Result<()> {
                        std::fs::rename(catalog, &moved)?;
                        std::fs::create_dir(catalog)?;
                        std::fs::set_permissions(catalog, std::fs::Permissions::from_mode(0o700))?;
                        Ok(())
                    })();
                });
            mutation?;
            if result.is_ok() || !visited {
                return Err("replaced Catalog directory was promoted".into());
            }
            Ok(())
        })
    }
}
