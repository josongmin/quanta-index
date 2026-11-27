//! The stock SQLite scratch namespace owns disposable files independently of
//! the durable publication inode ledger. The enclosing StateRoot supplies the
//! physical hard limit and the allocation supplies the logical reservation.

use std::{
    ffi::OsStr,
    fs::{self, File},
    os::unix::{ffi::OsStrExt as _, fs::MetadataExt as _},
    path::{Path, PathBuf},
    sync::Arc,
};

use quanta_index_core::{
    DiagnosticContextV1,
    index::{
        IndexError, ScratchIoBinding, ScratchIoScope, ScratchNamespaceLease, ScratchSqliteVfs,
        WorkBudget,
    },
    publication_capacity::InodeKey,
};
use rustix::fs::{AtFlags, Dir, Mode, OFlags};

use super::{
    FileScratchSpace, NamespaceLock, ScratchNamespaceIo, charge_cleanup_entry, validate_open_root,
    validate_private_metadata,
};

impl FileScratchSpace {
    /// Switch a freshly opened managed namespace to stock SQLite. Its existing
    /// exclusive lease stays held. The caller must have verified hard quota
    /// on the enclosing StateRoot and swept orphans through its boot ledger.
    pub fn into_quota_managed_stock(mut self) -> Result<Self, IndexError> {
        let inner = Arc::get_mut(&mut self.inner).ok_or_else(|| {
            IndexError::InvalidInput(
                "stock scratch must be selected before sharing the namespace".into(),
            )
        })?;
        if !matches!(inner._lock, NamespaceLock::Managed { .. }) {
            return Err(IndexError::InvalidInput(
                "stock scratch requires an admitted namespace lock".into(),
            ));
        }
        if *inner
            .reserved
            .get_mut()
            .map_err(|e| io("inspect scratch reservation", e))?
            != 0
        {
            return Err(IndexError::MaintenanceRequired);
        }
        validate_open_root(inner)?;
        inner.io = ScratchNamespaceIo::Stock(Arc::new(StockScratchIo {
            root: inner.root.clone(),
            directory: inner
                .root_file
                .try_clone()
                .map_err(|e| io("clone stock scratch root", e))?,
        }));
        Ok(self)
    }
}

#[derive(Debug)]
pub(super) struct StockScratchIo {
    root: PathBuf,
    directory: File,
}

struct StockScope;

impl ScratchIoScope for StockScope {
    fn is_capacity_refusal(&self, _backend_error_code: i32) -> bool {
        false
    }
}

impl ScratchIoBinding for StockScratchIo {
    fn driver_name(&self) -> &str {
        "unix"
    }

    fn sqlite_vfs(&self) -> ScratchSqliteVfs<'_> {
        ScratchSqliteVfs::Stock
    }

    fn enter_scope(&self, _diagnostics: Option<&DiagnosticContextV1>) -> Box<dyn ScratchIoScope> {
        Box::new(StockScope)
    }

    fn acquire_namespace(
        &self,
        _path: &Path,
    ) -> Result<Box<dyn ScratchNamespaceLease>, IndexError> {
        Err(IndexError::InvalidInput(
            "stock scratch requires the existing boot namespace lease".into(),
        ))
    }

    fn retire_namespace(
        &self,
        _path: &Path,
        _budget: Option<&mut WorkBudget>,
    ) -> Result<(), IndexError> {
        Err(IndexError::InvalidInput(
            "stock scratch retirement requires the allocation owner".into(),
        ))
    }
}

impl StockScratchIo {
    pub(super) fn retire_owned_allocation(
        &self,
        path: &Path,
        identity: InodeKey,
    ) -> Result<(), IndexError> {
        let name = path
            .file_name()
            .filter(|name| name.as_bytes().starts_with(b"alloc-"))
            .ok_or_else(|| {
                IndexError::InvalidInput("invalid stock scratch allocation name".into())
            })?;
        if path.parent() != Some(self.root.as_path()) {
            return Err(IndexError::InvalidInput(
                "foreign stock scratch allocation".into(),
            ));
        }
        let directory = open_directory(&self.directory, name)?;
        let metadata = directory
            .metadata()
            .map_err(|e| io("inspect stock scratch allocation", e))?;
        let device = self
            .directory
            .metadata()
            .map_err(|e| io("inspect stock scratch root", e))?
            .dev();
        validate_private_metadata(&metadata, device, true)?;
        if metadata.dev() != identity.device || metadata.ino() != identity.inode {
            return Err(IndexError::Storage(
                "stock scratch allocation changed before retirement".into(),
            ));
        }
        let mut entries = 0;
        remove_files(&directory, device, &mut entries)?;
        verify_name(&self.directory, name, &metadata)?;
        rustix::fs::unlinkat(&self.directory, name, AtFlags::REMOVEDIR)
            .map_err(|e| io("remove stock scratch allocation", e))?;
        rustix::fs::fsync(&self.directory).map_err(|e| io("sync stock scratch root", e))?;
        Ok(())
    }
}

fn open_directory(parent: &File, name: &OsStr) -> Result<File, IndexError> {
    rustix::fs::openat(
        parent,
        name,
        OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
        Mode::empty(),
    )
    .map(File::from)
    .map_err(|e| io("open stock scratch directory", e))
}

fn remove_files(directory: &File, device: u64, entries: &mut u64) -> Result<(), IndexError> {
    let mut scan = Dir::read_from(directory).map_err(|e| io("read stock scratch allocation", e))?;
    while let Some(entry) = scan.read() {
        let entry = entry.map_err(|e| io("read stock scratch entry", e))?;
        let name = entry.file_name().to_bytes();
        if name == b"." || name == b".." {
            continue;
        }
        charge_cleanup_entry(entries)?;
        let name = OsStr::from_bytes(name);
        let linked = rustix::fs::statat(directory, name, AtFlags::SYMLINK_NOFOLLOW)
            .map_err(|e| io("inspect stock scratch entry name", e))?;
        if rustix::fs::FileType::from_raw_mode(linked.st_mode) != rustix::fs::FileType::RegularFile
        {
            return Err(IndexError::Storage("unsafe stock scratch entry".into()));
        }
        let file = File::from(
            rustix::fs::openat(
                directory,
                name,
                OFlags::RDONLY | OFlags::NONBLOCK | OFlags::NOFOLLOW | OFlags::CLOEXEC,
                Mode::empty(),
            )
            .map_err(|e| io("open stock scratch entry", e))?,
        );
        let metadata = file
            .metadata()
            .map_err(|e| io("inspect stock scratch entry", e))?;
        if !metadata.is_file()
            || metadata.dev() != device
            || metadata.uid() != rustix::process::geteuid().as_raw()
            || metadata.nlink() != 1
            || metadata.mode() & 0o022 != 0
        {
            return Err(IndexError::Storage("unsafe stock scratch entry".into()));
        }
        verify_name(directory, name, &metadata)?;
        rustix::fs::unlinkat(directory, name, AtFlags::empty())
            .map_err(|e| io("unlink stock scratch entry", e))?;
    }
    Ok(())
}

fn verify_name(parent: &File, name: &OsStr, metadata: &fs::Metadata) -> Result<(), IndexError> {
    let linked = rustix::fs::statat(parent, name, AtFlags::SYMLINK_NOFOLLOW)
        .map_err(|e| io("inspect stock scratch name", e))?;
    if i128::from(linked.st_dev) != i128::from(metadata.dev()) || linked.st_ino != metadata.ino() {
        return Err(IndexError::Storage(
            "stock scratch name changed before retirement".into(),
        ));
    }
    Ok(())
}

fn io(operation: &str, error: impl std::fmt::Display) -> IndexError {
    IndexError::Storage(format!("{operation}: {error}"))
}
