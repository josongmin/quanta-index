//! Catalog digest input backed by the held StateRoot and stock SQLite files.

use super::*;
use crate::owned_fd_close::close_owned_file;
use crate::publication_file_capacity::{PublicationFileCapacity, checked_device_id};
use rustix::fs::{AtFlags, FileType, Mode, OFlags, openat, statat};
use std::fs::File;

struct StockInput<'a> {
    directory: &'a File,
    common: &'a PublicationFileCapacity,
    wal_name: &'a Path,
    observed_wal: bool,
}

impl StockInput<'_> {
    fn require_name(&self, name: &Path) -> Result<(), IndexError> {
        if name == Path::new(CATALOG_FILE_NAME) || name == self.wal_name {
            Ok(())
        } else {
            Err(IndexError::BackendViolation(
                "stock Catalog digest requested an unknown basename".into(),
            ))
        }
    }
}

impl DigestInput for StockInput<'_> {
    fn linked(&self, name: &Path, required: bool) -> Result<Option<FileIdentity>, IndexError> {
        self.require_name(name)?;
        if name == self.wal_name && !self.observed_wal {
            return if required {
                Err(IndexError::Storage(
                    "stock Catalog WAL was absent at anchored admission".into(),
                ))
            } else {
                Ok(None)
            };
        }
        let stat = match statat(self.directory, name, AtFlags::SYMLINK_NOFOLLOW) {
            Ok(stat) => stat,
            Err(rustix::io::Errno::NOENT) if !required => return Ok(None),
            Err(rustix::io::Errno::NOENT) => return Err(IndexError::SnapshotUnavailable),
            Err(error) => {
                return Err(IndexError::Storage(format!(
                    "inspect stock Catalog digest input: {error}"
                )));
            }
        };
        if FileType::from_raw_mode(stat.st_mode) != FileType::RegularFile
            || stat.st_nlink != 1
            || stat.st_uid != rustix::process::geteuid().as_raw()
            || stat.st_mode & 0o077 != 0
        {
            return Err(IndexError::Storage(
                "Catalog digest input must be a private, owned, single-link regular file".into(),
            ));
        }
        let bytes = u64::try_from(stat.st_size).map_err(|_error| {
            IndexError::Storage("stock Catalog digest input has a negative size".into())
        })?;
        Ok(Some(FileIdentity {
            device: checked_device_id(stat.st_dev)
                .map_err(|_| IndexError::Storage("stock Catalog device ID out of range".into()))?,
            inode: stat.st_ino,
            bytes,
            modified_seconds: stat.st_mtime,
            modified_nanos: i64::try_from(stat.st_mtime_nsec).map_err(|_| {
                IndexError::Storage("stock Catalog modified nanoseconds out of range".into())
            })?,
            changed_seconds: stat.st_ctime,
            changed_nanos: i64::try_from(stat.st_ctime_nsec).map_err(|_| {
                IndexError::Storage("stock Catalog changed nanoseconds out of range".into())
            })?,
        }))
    }

    fn with_file(
        &self,
        name: &Path,
        budget: &mut WorkBudget,
        read: &mut dyn FnMut(&File, &mut WorkBudget) -> Result<(), IndexError>,
    ) -> Result<(), IndexError> {
        self.require_name(name)?;
        if name == self.wal_name && !self.observed_wal {
            return Err(IndexError::Storage(
                "stock Catalog WAL was absent at anchored admission".into(),
            ));
        }
        budget.checkpoint()?;
        let file = File::from(
            openat(
                self.directory,
                name,
                OFlags::RDONLY | OFlags::CLOEXEC | OFlags::NOFOLLOW | OFlags::NONBLOCK,
                Mode::empty(),
            )
            .map_err(|error| {
                IndexError::Storage(format!("open stock Catalog digest input: {error}"))
            })?,
        );
        let result = read(&file, budget);
        if let Err(error) = close_owned_file(file) {
            self.common.mark_close_unwitnessed();
            return Err(IndexError::Storage(format!(
                "close stock Catalog digest input: {error}"
            )));
        }
        result
    }
}

/// Hash the exact Catalog main and optional WAL bytes under the leased root.
/// WAL presence and its descriptor slot are resolved under the anchored
/// directory before either digest input can be opened.
pub(super) fn catalog_content_digest(
    common: &PublicationFileCapacity,
    budget: &mut WorkBudget,
) -> Result<Digest, IndexError> {
    budget.checkpoint()?;
    let directory = catalog_dir(common.state_root());
    let wal_name = format!("{CATALOG_FILE_NAME}-wal");
    let path_bytes = directory
        .as_os_str()
        .len()
        .checked_add(wal_name.len())
        .ok_or(IndexError::ResourceLimited)?;
    let _paths = budget.reserve_memory(
        u64::try_from(path_bytes).map_err(|_error| IndexError::ResourceLimited)?,
        MemoryLifetime::Temporary,
    )?;
    budget.charge(WorkUsage {
        reads: 2,
        ..WorkUsage::default()
    })?;
    let request = budget.request().clone();
    let mut result = None;
    common.with_observed_catalog_directory_v1(
        &directory,
        &request,
        &mut |held, observed_wal| {
            let input = StockInput {
                directory: held,
                common,
                wal_name: Path::new(&wal_name),
                observed_wal,
            };
            result = Some(digest(&input, budget));
        },
    )?;
    result.ok_or_else(|| {
        IndexError::BackendViolation("stock Catalog digest directory callback was absent".into())
    })?
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::publication_inventory::{PublicationBootInventoryLimits, seed_publication_capacity};
    use quanta_index_core::{RequestBudgetV1, index::WorkLimits};
    use quanta_index_searchd::app::runtime::StateRootLease;
    use std::os::unix::fs::PermissionsExt as _;

    #[test]
    fn wal_born_after_observation_cannot_open_an_unreserved_fd()
    -> Result<(), Box<dyn std::error::Error>> {
        let temporary = tempfile::tempdir()?;
        let lease = StateRootLease::acquire(&temporary.path().join("state"))?;
        let root = lease.state_root_identity_v1().to_path_buf();
        let directory = catalog_dir(&root);
        fs::create_dir(&directory)?;
        fs::set_permissions(&directory, fs::Permissions::from_mode(0o700))?;
        let wal_name = format!("{CATALOG_FILE_NAME}-wal");
        let wal_path = directory.join(&wal_name);
        let capacity = seed_publication_capacity(
            &lease,
            &root,
            PublicationBootInventoryLimits {
                max_bytes: 1024,
                max_files: 8,
                max_pending: 8,
                max_entries: 16,
                max_depth: 4,
                max_pending_frames: 8,
            },
            &[],
            &RequestBudgetV1::unbounded(),
        )?;
        let common = PublicationFileCapacity::new(lease, capacity, 2, 16, 256)?;
        let held = File::open(directory)?;
        let input = StockInput {
            directory: &held,
            common: &common,
            wal_name: Path::new(&wal_name),
            observed_wal: false,
        };
        fs::write(&wal_path, b"born after admission")?;
        fs::set_permissions(&wal_path, fs::Permissions::from_mode(0o600))?;
        let wal = Path::new(&wal_name);
        if input.linked(wal, false)?.is_some() {
            return Err("unreserved WAL was exposed to digest".into());
        }
        let mut budget = WorkBudget::with_request(
            WorkLimits {
                reads: 16,
                writes: 0,
                bytes: 1024,
                mutations: 0,
            },
            RequestBudgetV1::unbounded(),
        );
        let mut called = false;
        let opened = input.with_file(wal, &mut budget, &mut |_file, _budget| {
            called = true;
            Ok(())
        });
        if opened.is_ok() || called {
            return Err("unreserved WAL opened after anchored observation".into());
        }
        Ok(())
    }
}
