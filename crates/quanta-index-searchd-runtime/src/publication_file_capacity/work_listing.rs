//! Anchored, bounded directory inventory for a held Semantic build.

use std::ffi::OsStr;
use std::fs::{File, Metadata};
use std::os::unix::ffi::OsStrExt as _;
use std::os::unix::fs::MetadataExt as _;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use quanta_index_core::CoreError;
use quanta_index_semantic::SemanticWorkListEntry;
use rustix::fs::{AtFlags, FileType, statat};

use super::{
    ListPrefixMode, PublicationFileCapacity, capacity, checked_device_id, contract, storage,
    storage_io,
};

#[derive(Default)]
struct DirectoryScan {
    identity: (u64, u64),
    entries: Vec<SemanticWorkListEntry>,
    children: Vec<(PathBuf, u64, u64)>,
}

fn same_directory(left: &Metadata, right: &Metadata) -> bool {
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

fn modified_time(
    seconds: impl TryInto<i64>,
    nanos: impl TryInto<i64>,
) -> Result<SystemTime, CoreError> {
    let seconds = seconds
        .try_into()
        .map_err(|_| contract("work listing file mtime seconds invalid"))?;
    let nanos = nanos
        .try_into()
        .map_err(|_| contract("work listing file mtime fraction invalid"))?;
    if !(0..1_000_000_000).contains(&nanos) {
        return Err(contract("work listing file mtime nanoseconds invalid"));
    }
    let total = i128::from(seconds)
        .checked_mul(1_000_000_000)
        .and_then(|value| value.checked_add(i128::from(nanos)))
        .ok_or_else(|| contract("work listing file mtime overflow"))?;
    let magnitude = total.unsigned_abs();
    let duration = Duration::new(
        u64::try_from(magnitude / 1_000_000_000)
            .map_err(|_| contract("work listing file mtime seconds overflow"))?,
        u32::try_from(magnitude % 1_000_000_000)
            .map_err(|_| contract("work listing file mtime fraction overflow"))?,
    );
    if total < 0 {
        UNIX_EPOCH.checked_sub(duration)
    } else {
        UNIX_EPOCH.checked_add(duration)
    }
    .ok_or_else(|| contract("work listing file mtime outside SystemTime"))
}

fn visit_names(
    held: &File,
    mut visit: impl FnMut(&[u8], u64) -> Result<(), CoreError>,
) -> Result<(), CoreError> {
    #[cfg(target_os = "linux")]
    {
        const BUFFER_BYTES: usize = 65_536;
        let mut buffer = Vec::<u8>::new();
        buffer
            .try_reserve_exact(BUFFER_BYTES)
            .map_err(|_| capacity("work listing directory buffer allocation failed"))?;
        let batch = buffer
            .spare_capacity_mut()
            .get_mut(..BUFFER_BYTES)
            .ok_or_else(|| capacity("work listing directory buffer undersized"))?;
        let mut entries = rustix::fs::RawDir::new(held, batch);
        while let Some(entry) = entries.next() {
            let entry = entry.map_err(|error| storage_io("read work directory", error))?;
            visit(entry.file_name().to_bytes(), entry.ino())?;
        }
    }
    #[cfg(not(target_os = "linux"))]
    {
        // rustix opens an independently positioned descriptor for this cursor.
        // Its safe Dir API is also used by the existing directory-capacity scan.
        let mut entries = rustix::fs::Dir::read_from(held)
            .map_err(|error| storage_io("open work directory cursor", error))?;
        while let Some(entry) = entries.read() {
            let entry = entry.map_err(|error| storage_io("read work directory", error))?;
            visit(entry.file_name().to_bytes(), entry.ino())?;
        }
    }
    Ok(())
}

impl PublicationFileCapacity {
    fn scan_work_directory(
        &self,
        prefix: &Path,
        held: &File,
        limit: usize,
    ) -> Result<DirectoryScan, CoreError> {
        let before = held
            .metadata()
            .map_err(|error| storage_io("stat held work directory", error))?;
        if !before.is_dir() {
            return Err(contract("work listing scope is not a directory"));
        }
        let mut scan = DirectoryScan {
            identity: (before.dev(), before.ino()),
            ..DirectoryScan::default()
        };
        visit_names(held, |bytes, inode| {
            if bytes == b"." || bytes == b".." {
                return Ok(());
            }
            if bytes.is_empty() || bytes.contains(&b'/') || bytes.contains(&0) {
                return Err(contract("work listing child basename invalid"));
            }
            if scan.entries.len() >= limit {
                return Err(capacity("work listing entry registry ceiling exceeded"));
            }
            let name = OsStr::from_bytes(bytes);
            let stat = statat(held, Path::new(name), AtFlags::SYMLINK_NOFOLLOW)
                .map_err(|error| storage_io("stat work listing child", error))?;
            if inode != 0 && inode != stat.st_ino {
                return Err(contract("work listing child incarnation changed"));
            }
            if stat.st_uid != rustix::process::getuid().as_raw() || stat.st_mode & 0o022 != 0 {
                return Err(contract("work listing child owner or mode invalid"));
            }
            let path = prefix.join(name);
            let entry = match FileType::from_raw_mode(stat.st_mode) {
                FileType::RegularFile => SemanticWorkListEntry::File {
                    path,
                    inode: stat.st_ino,
                    bytes: u64::try_from(stat.st_size)
                        .map_err(|_| contract("work listing file width invalid"))?,
                    modified: modified_time(stat.st_mtime, stat.st_mtime_nsec)?,
                },
                FileType::Directory => {
                    let device = checked_device_id(stat.st_dev)
                        .map_err(|error| storage("work listing directory device", error))?;
                    scan.children
                        .try_reserve(1)
                        .map_err(|_| capacity("work listing child allocation failed"))?;
                    scan.children.push((path.clone(), device, stat.st_ino));
                    SemanticWorkListEntry::Directory(path)
                }
                _ => return Err(contract("work listing contains unsupported inode type")),
            };
            scan.entries
                .try_reserve(1)
                .map_err(|_| capacity("work listing entry allocation failed"))?;
            scan.entries.push(entry);
            Ok(())
        })?;
        let after = held
            .metadata()
            .map_err(|error| storage_io("restat held work directory", error))?;
        if !same_directory(&before, &after) {
            return Err(contract("work listing directory changed during scan"));
        }
        Ok(scan)
    }

    fn scan_work_prefix(
        &self,
        prefix: &Path,
        expected: Option<(u64, u64)>,
        limit: usize,
    ) -> Result<Option<DirectoryScan>, CoreError> {
        let scanned = self.with_list_prefix(prefix, ListPrefixMode::DirectoryOrEmpty, |held| {
            let Some(held) = held else {
                return Ok(None);
            };
            if let Some((device, inode)) = expected {
                let metadata = held
                    .metadata()
                    .map_err(|error| storage_io("stat expected work directory", error))?;
                if metadata.dev() != device || metadata.ino() != inode {
                    return Err(contract("work listing directory incarnation changed"));
                }
            }
            self.scan_work_directory(prefix, held, limit).map(Some)
        })?;
        if let Some(scanned) = &scanned {
            // The held FD can outlive a renamed directory. Reopen its
            // current pathname before publishing its inventory.
            self.with_existing_list_scope_v1(prefix, true, |linked| {
                let metadata = linked
                    .metadata()
                    .map_err(|error| storage_io("stat linked work directory", error))?;
                if !metadata.is_dir() || (metadata.dev(), metadata.ino()) != scanned.identity {
                    return Err(contract("work listing directory link changed"));
                }
                Ok::<(), CoreError>(())
            })?;
        }
        Ok(scanned)
    }

    /// Inventory an already scoped work dataset prefix without following links.
    /// A missing or regular-file prefix has no descendants.
    pub(crate) fn list_work_dataset(
        &self,
        prefix: &Path,
        recursive: bool,
    ) -> Result<Vec<SemanticWorkListEntry>, CoreError> {
        if !prefix.is_absolute() || prefix == self.state_root() {
            return Err(contract("work listing requires an absolute dataset prefix"));
        }
        self.shared.root.require_current_v1()?;
        let limit = self.shared.max_tracked_entries;
        let Some(first) = self.scan_work_prefix(prefix, None, limit)? else {
            return Ok(Vec::new());
        };
        let mut result = first.entries;
        let mut pending = if recursive {
            first.children
        } else {
            Vec::new()
        };
        while let Some((directory, device, inode)) = pending.pop() {
            let remaining = limit
                .checked_sub(result.len())
                .ok_or_else(|| capacity("work listing entry registry ceiling exceeded"))?;
            let scanned = self
                .scan_work_prefix(&directory, Some((device, inode)), remaining)?
                .ok_or_else(|| contract("work listing child directory disappeared"))?;
            pending
                .try_reserve(scanned.children.len())
                .map_err(|_| capacity("work listing stack allocation failed"))?;
            pending.extend(scanned.children);
            result
                .try_reserve(scanned.entries.len())
                .map_err(|_| capacity("work listing result allocation failed"))?;
            result.extend(scanned.entries);
        }
        result.sort_unstable_by(|left, right| listed_path(left).cmp(listed_path(right)));
        self.shared.root.require_current_v1()?;
        Ok(result)
    }
}

fn listed_path(entry: &SemanticWorkListEntry) -> &Path {
    match entry {
        SemanticWorkListEntry::File { path, .. } | SemanticWorkListEntry::Directory(path) => path,
    }
}
