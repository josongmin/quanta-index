//! Process-lifetime ownership of one immutable object directory.
//!
//! This protects managed and standalone openers from one another. Physical
//! deletion still needs a separate catalog-custody and write/reclaim gate.

use std::{
    fs::{self, File, TryLockError},
    os::unix::fs::MetadataExt,
    path::{Path, PathBuf},
};

use quanta_index_core::index::{IndexError, ObjectNamespaceBinding, ObjectReadFile};
use rustix::fs::{AtFlags, Mode, OFlags};

const LEASE_NAME: &str = ".object-namespace.lease";

// A namespace owns an immutable shared reader, never a mutable batch writer.
enum NamespaceFile {
    Plain(File),
    Admitted(Box<dyn ObjectReadFile>),
}
impl NamespaceFile {
    fn file(&self) -> Result<&File, IndexError> {
        match self {
            Self::Plain(file) => Ok(file),
            Self::Admitted(owner) => owner.file(),
        }
    }
}

pub(super) struct NamespaceLease {
    root_path: PathBuf,
    root: File,
    lock: NamespaceFile,
    objects: Option<File>,
}

impl NamespaceLease {
    pub(super) fn shared(root: &Path) -> Result<Self, IndexError> {
        Self::open(root, false, None, true)
    }

    pub(super) fn exclusive(root: &Path) -> Result<Self, IndexError> {
        Self::open(root, true, None, true)
    }

    pub(super) fn exclusive_with_io(
        root: &Path,
        lock: Box<dyn ObjectReadFile>,
    ) -> Result<Self, IndexError> {
        Self::open(root, true, Some(lock), true)
    }

    pub(super) fn existing_with_io(
        root: &Path,
        lock: Box<dyn ObjectReadFile>,
    ) -> Result<Self, IndexError> {
        Self::open(root, true, Some(lock), false)
    }

    fn open(
        root: &Path,
        exclusive: bool,
        admitted: Option<Box<dyn ObjectReadFile>>,
        create: bool,
    ) -> Result<Self, IndexError> {
        let linked_root = fs::symlink_metadata(root)
            .map_err(|error| IndexError::Storage(format!("inspect object root: {error}")))?;
        validate_private_root(&linked_root)?;
        let root_file = File::from(
            rustix::fs::open(
                root,
                OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
                Mode::empty(),
            )
            .map_err(|error| IndexError::Storage(format!("open object root: {error}")))?,
        );
        let opened_root = root_file
            .metadata()
            .map_err(|error| IndexError::Storage(format!("inspect held object root: {error}")))?;
        validate_private_root(&opened_root)?;
        if opened_root.dev() != linked_root.dev() || opened_root.ino() != linked_root.ino() {
            return Err(IndexError::Storage(
                "object root changed during namespace open".into(),
            ));
        }

        let lock = if let Some(owner) = admitted {
            NamespaceFile::Admitted(owner)
        } else {
            NamespaceFile::Plain(File::from(
                rustix::fs::openat(
                    &root_file,
                    LEASE_NAME,
                    OFlags::RDWR
                        | OFlags::NOFOLLOW
                        | OFlags::CLOEXEC
                        | if create {
                            OFlags::CREATE
                        } else {
                            OFlags::empty()
                        },
                    Mode::from_raw_mode(0o600),
                )
                .map_err(|error| {
                    IndexError::Storage(format!("open object namespace lease: {error}"))
                })?,
            ))
        };
        let opened_lock = lock.file()?.metadata().map_err(|error| {
            IndexError::Storage(format!("inspect object namespace lease: {error}"))
        })?;
        validate_private_lease(&opened_lock, opened_root.dev())?;
        let linked_lock = rustix::fs::statat(&root_file, LEASE_NAME, AtFlags::SYMLINK_NOFOLLOW)
            .map_err(|error| {
                IndexError::Storage(format!("inspect object namespace lease name: {error}"))
            })?;
        let held_lock = rustix::fs::fstat(lock.file()?).map_err(|error| {
            IndexError::Storage(format!("inspect held object namespace lease: {error}"))
        })?;
        if linked_lock.st_dev != held_lock.st_dev || linked_lock.st_ino != held_lock.st_ino {
            return Err(IndexError::Storage(
                "object namespace lease changed during open".into(),
            ));
        }
        let lock_result = if exclusive {
            lock.file()?.try_lock()
        } else {
            lock.file()?.try_lock_shared()
        };
        match lock_result {
            Ok(()) => {}
            Err(TryLockError::WouldBlock) => return Err(IndexError::ResourceLimited),
            Err(TryLockError::Error(error)) => {
                return Err(IndexError::Storage(format!(
                    "lock object namespace: {error}"
                )));
            }
        }
        // The path must still name the locked inode after acquisition. A
        // replaced name could otherwise admit an independent second owner.
        let linked_after = rustix::fs::statat(&root_file, LEASE_NAME, AtFlags::SYMLINK_NOFOLLOW)
            .map_err(|error| {
                IndexError::Storage(format!("reinspect object namespace lease: {error}"))
            })?;
        if linked_after.st_dev != held_lock.st_dev || linked_after.st_ino != held_lock.st_ino {
            return Err(IndexError::Storage(
                "object namespace lease changed during lock".into(),
            ));
        }
        if create {
            lock.file()?.sync_all().map_err(|error| {
                IndexError::Storage(format!("sync object namespace lease: {error}"))
            })?;
            root_file.sync_all().map_err(|error| {
                IndexError::Storage(format!("sync object namespace root: {error}"))
            })?;
        }
        Ok(Self {
            root_path: root.to_path_buf(),
            root: root_file,
            lock,
            objects: None,
        })
    }

    pub(super) fn bind_objects(&mut self, objects: &Path) -> Result<(), IndexError> {
        let file = File::from(
            rustix::fs::open(
                objects,
                OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
                Mode::empty(),
            )
            .map_err(|error| IndexError::Storage(format!("open held object directory: {error}")))?,
        );
        let linked = fs::symlink_metadata(objects)
            .map_err(|error| IndexError::Storage(format!("inspect object directory: {error}")))?;
        let opened = file.metadata().map_err(|error| {
            IndexError::Storage(format!("inspect held object directory: {error}"))
        })?;
        validate_private_root(&linked)?;
        validate_private_root(&opened)?;
        if linked.dev() != opened.dev() || linked.ino() != opened.ino() {
            return Err(IndexError::Storage(
                "object directory changed during open".into(),
            ));
        }
        self.objects = Some(file);
        let _binding = self.validate_objects(objects)?;
        Ok(())
    }

    pub(super) fn validate_objects(
        &self,
        objects: &Path,
    ) -> Result<ObjectNamespaceBinding, IndexError> {
        let held_root = self
            .root
            .metadata()
            .map_err(|error| IndexError::Storage(format!("inspect held root: {error}")))?;
        let linked_root = fs::symlink_metadata(&self.root_path)
            .map_err(|error| IndexError::Storage(format!("inspect root: {error}")))?;
        validate_private_root(&held_root)?;
        validate_private_root(&linked_root)?;
        if held_root.dev() != linked_root.dev() || held_root.ino() != linked_root.ino() {
            return Err(IndexError::Storage("object root name changed".into()));
        }
        let held_lock = self.lock.file()?.metadata().map_err(|error| {
            IndexError::Storage(format!("inspect held namespace lease: {error}"))
        })?;
        let linked_lock = fs::symlink_metadata(self.root_path.join(LEASE_NAME))
            .map_err(|error| IndexError::Storage(format!("inspect namespace lease: {error}")))?;
        validate_private_lease(&held_lock, held_root.dev())?;
        validate_private_lease(&linked_lock, held_root.dev())?;
        if held_lock.dev() != linked_lock.dev() || held_lock.ino() != linked_lock.ino() {
            return Err(IndexError::Storage(
                "object namespace lease name changed".into(),
            ));
        }
        let held_objects = self
            .objects
            .as_ref()
            .ok_or_else(|| {
                IndexError::BackendViolation("managed object directory is not bound".into())
            })?
            .metadata()
            .map_err(|error| IndexError::Storage(format!("inspect held objects: {error}")))?;
        let linked_objects = fs::symlink_metadata(objects)
            .map_err(|error| IndexError::Storage(format!("inspect objects: {error}")))?;
        validate_private_root(&held_objects)?;
        validate_private_root(&linked_objects)?;
        if held_objects.dev() != linked_objects.dev() || held_objects.ino() != linked_objects.ino()
        {
            return Err(IndexError::Storage("object directory name changed".into()));
        }
        Ok(ObjectNamespaceBinding {
            root_device: held_root.dev(),
            root_inode: held_root.ino(),
            objects_device: held_objects.dev(),
            objects_inode: held_objects.ino(),
            lease_device: held_lock.dev(),
            lease_inode: held_lock.ino(),
        })
    }

    pub(super) fn held_objects(&self) -> Result<&File, IndexError> {
        self.objects.as_ref().ok_or_else(|| {
            IndexError::BackendViolation("managed object directory is not bound".into())
        })
    }
}

pub(super) fn validate_private_root(metadata: &fs::Metadata) -> Result<(), IndexError> {
    if !metadata.file_type().is_dir()
        || metadata.uid() != rustix::process::geteuid().as_raw()
        || metadata.mode() & 0o7777 != 0o700
    {
        return Err(IndexError::Storage(
            "object root has an unsafe owner, mode or type".into(),
        ));
    }
    Ok(())
}

fn validate_private_lease(metadata: &fs::Metadata, root_device: u64) -> Result<(), IndexError> {
    if !metadata.file_type().is_file()
        || metadata.nlink() != 1
        || metadata.dev() != root_device
        || metadata.uid() != rustix::process::geteuid().as_raw()
        || metadata.mode() & 0o7777 != 0o600
    {
        return Err(IndexError::Storage(
            "object namespace lease has an unsafe owner, mode, type, link or device".into(),
        ));
    }
    Ok(())
}
