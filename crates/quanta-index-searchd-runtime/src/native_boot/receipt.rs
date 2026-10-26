//! A native installation receipt is published only through its retained parent FD.

use std::{
    ffi::{OsStr, OsString},
    fs::{self, File},
    io::{self, Write as _},
    os::unix::fs::{MetadataExt as _, PermissionsExt as _},
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
};

use anyhow::{Context as _, Result, anyhow, bail};
use quanta_index_core::index::{Digest, IndexError, MemoryLifetime, WorkBudget, WorkUsage};
use quanta_index_searchd::cli::NativeBootIdentityV1;
use rusqlite::ffi::quanta_capacity_vfs::close_owned_file;
use rustix::fs::{AtFlags, Mode, OFlags, linkat, open, openat, statat, unlinkat};
use serde::Serialize;

const MAX_RECEIPT_BYTES: u64 = 4096;
const TEMP_ATTEMPTS: usize = 32;
static TEMP_SEQUENCE: AtomicU64 = AtomicU64::new(0);

#[derive(Serialize)]
struct Receipt {
    root_nonce_hex: String,
    manifest_operation: quanta_index_core::index::OperationKey,
    manifest_object_hex: String,
    source_cut_digest_hex: String,
    install_catalog_digest_hex: String,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct Inode {
    device: u64,
    number: u64,
}

fn inode(metadata: &fs::Metadata) -> Inode {
    Inode {
        device: metadata.dev(),
        number: metadata.ino(),
    }
}

fn hex(bytes: &[u8; 32]) -> String {
    const DIGITS: &[u8] = b"0123456789abcdef";
    let mut out = String::with_capacity(64);
    for byte in bytes {
        for digit in [byte >> 4, byte & 15] {
            if let Some(value) = DIGITS.get(usize::from(digit)) {
                out.push(char::from(*value));
            }
        }
    }
    out
}

fn entry(dir: &File, name: &OsStr) -> Result<Option<Inode>> {
    match statat(dir, Path::new(name), AtFlags::SYMLINK_NOFOLLOW) {
        Ok(value) => Ok(Some(Inode {
            device: crate::publication_inventory::checked_device_id(value.st_dev)
                .map_err(|_error| anyhow!("native receipt device number overflow"))?,
            number: value.st_ino,
        })),
        Err(rustix::io::Errno::NOENT) => Ok(None),
        Err(error) => Err(anyhow!("inspect native receipt entry: {error}")),
    }
}

fn close(file: File, name: &'static str) -> Result<()> {
    close_owned_file(file).with_context(|| format!("close {name}"))
}

/// The directory identity is retained before Catalog selection mutates state.
pub(super) struct ReceiptTarget {
    original_parent: PathBuf,
    canonical_parent: PathBuf,
    parent: Option<File>,
    parent_inode: Inode,
    name: OsString,
}

pub(super) fn target(path: &Path, state_root: &Path) -> Result<ReceiptTarget> {
    if !path.is_absolute() {
        bail!("native receipt path must be absolute");
    }
    let original_parent = path
        .parent()
        .ok_or_else(|| anyhow!("native receipt has no parent"))?;
    let name = path
        .file_name()
        .ok_or_else(|| anyhow!("native receipt has no name"))?;
    if !matches!(
        Path::new(name).components().next(),
        Some(std::path::Component::Normal(_))
    ) || Path::new(name).components().count() != 1
    {
        bail!("native receipt name is not a single filename");
    }
    if fs::symlink_metadata(original_parent)?
        .file_type()
        .is_symlink()
    {
        bail!("native receipt parent must not be a symlink");
    }
    let canonical_parent = fs::canonicalize(original_parent)?;
    if canonical_parent.starts_with(state_root) {
        bail!("native receipt parent must be outside the state root");
    }
    let parent = File::from(open(
        &canonical_parent,
        OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
        Mode::empty(),
    )?);
    let metadata = parent.metadata()?;
    if !metadata.is_dir()
        || metadata.uid() != rustix::process::geteuid().as_raw()
        || metadata.mode() & 0o022 != 0
        || fs::symlink_metadata(&canonical_parent)?
            .file_type()
            .is_symlink()
        || inode(&fs::symlink_metadata(&canonical_parent)?) != inode(&metadata)
    {
        bail!("native receipt parent must be owned, protected and stable");
    }
    if entry(&parent, name)?.is_some() {
        bail!("native receipt target already exists");
    }
    Ok(ReceiptTarget {
        original_parent: original_parent.to_path_buf(),
        canonical_parent,
        parent: Some(parent),
        parent_inode: inode(&metadata),
        name: name.to_os_string(),
    })
}

impl ReceiptTarget {
    fn parent(&self) -> Result<&File> {
        self.parent
            .as_ref()
            .ok_or_else(|| anyhow!("native receipt directory FD is closed"))
    }

    fn check_parent(&self) -> Result<()> {
        if fs::canonicalize(&self.original_parent)? != self.canonical_parent
            || fs::symlink_metadata(&self.original_parent)?
                .file_type()
                .is_symlink()
        {
            bail!("native receipt parent path changed");
        }
        let linked = fs::symlink_metadata(&self.canonical_parent)?;
        let opened = self.parent()?.metadata()?;
        if !linked.is_dir()
            || inode(&linked) != self.parent_inode
            || inode(&opened) != self.parent_inode
            || linked.uid() != rustix::process::geteuid().as_raw()
            || linked.mode() & 0o022 != 0
        {
            bail!("native receipt directory identity changed");
        }
        Ok(())
    }

    fn create_temp(&self) -> Result<(OsString, File, Inode)> {
        for _ in 0..TEMP_ATTEMPTS {
            let number = TEMP_SEQUENCE.fetch_add(1, Ordering::Relaxed);
            let name = OsString::from(format!(
                ".native-receipt-{}-{number:016x}",
                std::process::id()
            ));
            match openat(
                self.parent()?,
                Path::new(&name),
                OFlags::WRONLY | OFlags::CREATE | OFlags::EXCL | OFlags::NOFOLLOW | OFlags::CLOEXEC,
                Mode::from_raw_mode(0o600),
            ) {
                Ok(fd) => {
                    let file = File::from(fd);
                    let first = match file.metadata() {
                        Ok(metadata) => metadata,
                        Err(error) => {
                            let closed = close(file, "native receipt temporary file");
                            return Err(anyhow!(
                                "native receipt postcreation uncertainty: cannot identify temporary inode: {error}; close: {closed:?}"
                            ));
                        }
                    };
                    let own_inode = inode(&first);
                    let setup = (|| -> Result<()> {
                        file.set_permissions(fs::Permissions::from_mode(0o600))?;
                        let metadata = file.metadata()?;
                        if !metadata.is_file()
                            || metadata.nlink() != 1
                            || inode(&metadata) != own_inode
                            || metadata.mode() & 0o777 != 0o600
                            || entry(self.parent()?, &name)? != Some(own_inode)
                        {
                            bail!("native receipt temporary file is not private regular storage");
                        }
                        Ok(())
                    })();
                    if let Err(error) = setup {
                        let closed = close(file, "native receipt temporary file");
                        let cleanup = self
                            .unlink_owned(&name, own_inode)
                            .and_then(|()| self.parent()?.sync_all().map_err(Into::into));
                        if closed.is_err() || cleanup.is_err() {
                            return Err(anyhow!(
                                "native receipt postcreation uncertainty: {error}; close: {closed:?}; cleanup: {cleanup:?}"
                            ));
                        }
                        return Err(error);
                    }
                    return Ok((name, file, own_inode));
                }
                Err(rustix::io::Errno::EXIST) => {}
                Err(error) => return Err(anyhow!("create native receipt temporary file: {error}")),
            }
        }
        bail!("native receipt temporary names exhausted")
    }

    fn unlink_owned(&self, name: &OsStr, expected: Inode) -> Result<()> {
        match entry(self.parent()?, name)? {
            None => Ok(()),
            Some(actual) if actual == expected => {
                unlinkat(self.parent()?, Path::new(name), AtFlags::empty())?;
                Ok(())
            }
            Some(_) => bail!("native receipt entry changed; refusing to remove another inode"),
        }
    }

    /// Consume the target exactly once. A reported postpublication failure
    /// attempts inode-checked removal and reports uncertainty if cleanup fails.
    pub(super) fn write(
        self,
        boot: NativeBootIdentityV1,
        digest: Digest,
        budget: &mut WorkBudget,
    ) -> Result<()> {
        self.write_with_io(boot, digest, budget, File::sync_all, close)
    }

    #[cfg(test)]
    fn write_with_sync(
        self,
        boot: NativeBootIdentityV1,
        digest: Digest,
        budget: &mut WorkBudget,
        sync: impl FnMut(&File) -> io::Result<()>,
    ) -> Result<()> {
        self.write_with_io(boot, digest, budget, sync, close)
    }

    fn write_with_io(
        mut self,
        boot: NativeBootIdentityV1,
        digest: Digest,
        budget: &mut WorkBudget,
        mut sync: impl FnMut(&File) -> io::Result<()>,
        mut close_file: impl FnMut(File, &'static str) -> Result<()>,
    ) -> Result<()> {
        self.check_parent()?;
        let _backing = budget.reserve_memory(MAX_RECEIPT_BYTES, MemoryLifetime::Temporary)?;
        let receipt = Receipt {
            root_nonce_hex: hex(&boot.root_nonce),
            manifest_operation: boot.manifest_operation,
            manifest_object_hex: hex(&(boot.manifest_object.0).0),
            source_cut_digest_hex: hex(&boot.source_cut_digest.0),
            install_catalog_digest_hex: hex(&digest.0),
        };
        let bytes = serde_json::to_vec(&receipt)?;
        let length = u64::try_from(bytes.len())?;
        if length > MAX_RECEIPT_BYTES {
            return Err(IndexError::ResourceLimited.into());
        }
        budget.charge(WorkUsage {
            writes: 2,
            bytes: length,
            mutations: 1,
            ..Default::default()
        })?;
        let (temporary, file, own_inode) = self.create_temp()?;
        let mut file = Some(file);
        let mut published = false;
        let work = (|| -> Result<()> {
            let active = file
                .as_mut()
                .ok_or_else(|| anyhow!("native receipt temporary FD missing"))?;
            active.write_all(&bytes)?;
            sync(active)?;
            if inode(&active.metadata()?) != own_inode
                || entry(self.parent()?, &temporary)? != Some(own_inode)
            {
                bail!("native receipt temporary file changed");
            }
            close_file(
                file.take()
                    .ok_or_else(|| anyhow!("native receipt temporary FD missing"))?,
                "native receipt temporary file",
            )?;
            budget.checkpoint()?;
            self.check_parent()?;
            if entry(self.parent()?, &temporary)? != Some(own_inode)
                || entry(self.parent()?, &self.name)?.is_some()
            {
                bail!("native receipt path changed before publication");
            }
            linkat(
                self.parent()?,
                Path::new(&temporary),
                self.parent()?,
                Path::new(&self.name),
                AtFlags::empty(),
            )?;
            published = true;
            if entry(self.parent()?, &self.name)? != Some(own_inode) {
                bail!("native receipt publication identity changed");
            }
            self.unlink_owned(&temporary, own_inode)?;
            sync(self.parent()?)?;
            self.check_parent()?;
            if entry(self.parent()?, &self.name)? != Some(own_inode) {
                bail!("native receipt target changed after directory sync");
            }
            budget.checkpoint()?;
            Ok(())
        })();
        if let Err(error) = work {
            let file_close = file
                .take()
                .map(|file| close_file(file, "native receipt temporary file"));
            // Attempt independent cleanup even when a replacement prevents
            // deleting the final name. Never unlink another inode.
            let published_cleanup = if published {
                self.unlink_owned(&self.name, own_inode)
            } else {
                Ok(())
            };
            let temporary_cleanup = self.unlink_owned(&temporary, own_inode);
            let sync_cleanup = self
                .parent()
                .and_then(|parent| parent.sync_all().map_err(Into::into));
            let cleanup = published_cleanup.and(temporary_cleanup).and(sync_cleanup);
            let closed = self
                .parent
                .take()
                .map(|file| close_file(file, "native receipt directory"));
            if let Err(cleanup) = cleanup {
                return Err(anyhow!(
                    "native receipt postcommit uncertainty: {error}; cleanup: {cleanup}"
                ));
            }
            if let Some(Err(close_error)) = file_close {
                return Err(anyhow!(
                    "native receipt failure: {error}; temporary close uncertain: {close_error}"
                ));
            }
            if let Some(Err(close_error)) = closed {
                return Err(anyhow!(
                    "native receipt failure: {error}; directory close uncertain: {close_error}"
                ));
            }
            return Err(error);
        }
        let parent = self
            .parent
            .take()
            .ok_or_else(|| anyhow!("native receipt directory FD missing"))?;
        close_file(parent, "native receipt directory")
            .context("native receipt postcommit uncertainty after durable publication")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use anyhow::ensure;
    use quanta_index_core::RequestBudgetV1;
    use quanta_index_core::index::{
        CollectionId, ObjectRef, OperationKey, ProducerId, SlotId, StreamId, TenantId, WorkLimits,
        WorkspaceKey, WorktreeId,
    };
    use std::time::{Duration, Instant};

    fn budget() -> Result<WorkBudget> {
        Ok(WorkBudget::new(
            WorkLimits {
                reads: 1_000,
                writes: 1_000,
                bytes: 1024 * 1024,
                mutations: 100,
            },
            Instant::now()
                .checked_add(Duration::from_secs(30))
                .ok_or_else(|| anyhow!("test deadline overflow"))?,
        ))
    }

    fn boot() -> NativeBootIdentityV1 {
        NativeBootIdentityV1 {
            root_nonce: [1; 32],
            manifest_operation: OperationKey {
                workspace: WorkspaceKey {
                    tenant: TenantId([2; 16]),
                    collection: CollectionId([3; 16]),
                    worktree: WorktreeId([4; 16]),
                    slot: SlotId([5; 16]),
                },
                epoch: 6,
                producer: ProducerId([7; 16]),
                stream: StreamId([8; 16]),
                stream_epoch: 9,
                sequence: 10,
            },
            manifest_object: ObjectRef(Digest([11; 32])),
            source_cut_digest: Digest([12; 32]),
        }
    }

    #[test]
    fn target_refuses_existing_and_temp_is_private() -> Result<()> {
        let root = tempfile::tempdir()?;
        let output = tempfile::tempdir()?;
        let path = output.path().join("receipt.json");
        let prepared = target(&path, root.path())?;
        let (name, file, owned) = prepared.create_temp()?;
        let metadata = file.metadata()?;
        ensure!(metadata.mode() & 0o777 == 0o600, "temporary mode differs");
        ensure!(
            entry(prepared.parent()?, &name)? == Some(owned),
            "temporary inode differs"
        );
        close(file, "test receipt temporary file")?;
        prepared.unlink_owned(&name, owned)?;
        fs::write(&path, b"existing")?;
        ensure!(
            target(&path, root.path()).is_err(),
            "existing target accepted"
        );
        Ok(())
    }

    #[test]
    fn receipt_publishes_once_with_private_mode_and_exact_fields() -> Result<()> {
        let root = tempfile::tempdir()?;
        let output = tempfile::tempdir()?;
        let path = output.path().join("receipt.json");
        target(&path, root.path())?.write(boot(), Digest([13; 32]), &mut budget()?)?;
        ensure!(
            fs::symlink_metadata(&path)?.mode() & 0o777 == 0o600,
            "receipt mode differs"
        );
        let payload: serde_json::Value = serde_json::from_slice(&fs::read(&path)?)?;
        let field_count = payload
            .as_object()
            .ok_or_else(|| anyhow!("receipt is not an object"))?
            .len();
        ensure!(field_count == 5, "receipt field count differs");
        ensure!(
            target(&path, root.path()).is_err(),
            "published target accepted"
        );
        Ok(())
    }

    #[test]
    fn final_directory_close_failure_preserves_durable_receipt_and_refuses_retry() -> Result<()> {
        let root = tempfile::tempdir()?;
        let output = tempfile::tempdir()?;
        let path = output.path().join("receipt.json");
        let mut synced = 0;
        let mut closes = Vec::new();
        let result = target(&path, root.path())?.write_with_io(
            boot(),
            Digest([13; 32]),
            &mut budget()?,
            |file| {
                file.sync_all()?;
                synced += 1;
                Ok(())
            },
            |file, label| {
                closes.push(label);
                // Consume the actual FD once; report a fault at the final
                // close boundary without leaking or closing a reused FD.
                close(file, label)?;
                if label == "native receipt directory" {
                    return Err(
                        io::Error::from_raw_os_error(rustix::io::Errno::IO.raw_os_error()).into(),
                    );
                }
                Ok(())
            },
        );
        let error = result
            .err()
            .ok_or_else(|| anyhow!("final close fault succeeded"))?;
        ensure!(
            error.to_string() == "native receipt postcommit uncertainty after durable publication",
            "final close did not retain its postcommit classification"
        );
        let cause = error
            .downcast_ref::<io::Error>()
            .ok_or_else(|| anyhow!("final close lost the IO cause"))?;
        ensure!(
            cause.raw_os_error() == Some(rustix::io::Errno::IO.raw_os_error()),
            "final close cause differs"
        );
        ensure!(synced == 2, "receipt was not file/directory synced");
        ensure!(
            closes == ["native receipt temporary file", "native receipt directory"],
            "owned descriptors were not closed exactly once"
        );
        let payload: serde_json::Value = serde_json::from_slice(&fs::read(&path)?)?;
        let expected = serde_json::json!({
            "root_nonce_hex": "01".repeat(32),
            "manifest_operation": {
                "workspace": {
                    "tenant": vec![2_u8; 16], "collection": vec![3_u8; 16],
                    "worktree": vec![4_u8; 16], "slot": vec![5_u8; 16]
                },
                "epoch": 6, "producer": vec![7_u8; 16], "stream": vec![8_u8; 16],
                "stream_epoch": 9, "sequence": 10
            },
            "manifest_object_hex": "0b".repeat(32),
            "source_cut_digest_hex": "0c".repeat(32),
            "install_catalog_digest_hex": "0d".repeat(32)
        });
        ensure!(
            payload == expected,
            "durable receipt differs from fixed wire gold"
        );
        let before = fs::symlink_metadata(&path)?;
        ensure!(
            before.mode() & 0o777 == 0o600 && before.nlink() == 1,
            "durable receipt is not private single-link storage"
        );
        ensure!(
            fs::read_dir(output.path())?.count() == 1,
            "close fault left a temporary entry"
        );
        ensure!(
            target(&path, root.path()).is_err(),
            "uncertain durable receipt permitted a blind retry"
        );
        let after = fs::symlink_metadata(&path)?;
        ensure!(
            inode(&before) == inode(&after),
            "retry changed receipt identity"
        );
        ensure!(
            serde_json::from_slice::<serde_json::Value>(&fs::read(&path)?)? == expected,
            "refused retry changed receipt bytes"
        );
        Ok(())
    }

    #[test]
    fn failed_directory_sync_removes_published_receipt() -> Result<()> {
        let root = tempfile::tempdir()?;
        let output = tempfile::tempdir()?;
        let path = output.path().join("receipt.json");
        let mut calls = 0;
        let result = target(&path, root.path())?.write_with_sync(
            boot(),
            Digest([13; 32]),
            &mut budget()?,
            |file| {
                calls += 1;
                if calls == 2 {
                    Err(io::Error::other("injected directory sync failure"))
                } else {
                    file.sync_all()
                }
            },
        );
        ensure!(result.is_err(), "directory sync refusal succeeded");
        ensure!(!path.exists(), "failed receipt remains");
        ensure!(
            fs::read_dir(output.path())?.count() == 0,
            "failed receipt left directory entries"
        );
        Ok(())
    }

    #[test]
    fn failed_directory_sync_does_not_remove_replacement_inode() -> Result<()> {
        let root = tempfile::tempdir()?;
        let output = tempfile::tempdir()?;
        let path = output.path().join("receipt.json");
        let replacement = output.path().join("replacement");
        fs::write(&replacement, b"other inode")?;
        let mut calls = 0;
        let result = target(&path, root.path())?.write_with_sync(
            boot(),
            Digest([13; 32]),
            &mut budget()?,
            |file| {
                calls += 1;
                if calls == 2 {
                    fs::rename(&replacement, &path)?;
                    Err(io::Error::other("injected directory sync failure"))
                } else {
                    file.sync_all()
                }
            },
        );
        ensure!(result.is_err(), "directory sync refusal succeeded");
        ensure!(
            fs::read(&path)? == b"other inode",
            "replacement bytes differ"
        );
        ensure!(!replacement.exists(), "replacement source remains");
        Ok(())
    }

    #[test]
    fn cancellation_after_publication_removes_owned_receipt() -> Result<()> {
        let root = tempfile::tempdir()?;
        let output = tempfile::tempdir()?;
        let path = output.path().join("receipt.json");
        let request = RequestBudgetV1::for_duration(Duration::from_secs(30));
        let cancel = request.cancel_handle();
        let mut work = WorkBudget::with_request(
            WorkLimits {
                reads: 1_000,
                writes: 1_000,
                bytes: 1024 * 1024,
                mutations: 100,
            },
            request,
        );
        let mut calls = 0;
        let result = target(&path, root.path())?.write_with_sync(
            boot(),
            Digest([13; 32]),
            &mut work,
            |file| {
                calls += 1;
                file.sync_all()?;
                if calls == 2 {
                    cancel.cancel();
                }
                Ok(())
            },
        );
        ensure!(result.is_err(), "canceled receipt succeeded");
        ensure!(!path.exists(), "canceled receipt remains");
        ensure!(
            fs::read_dir(output.path())?.count() == 0,
            "canceled receipt left directory entries"
        );
        Ok(())
    }
}
