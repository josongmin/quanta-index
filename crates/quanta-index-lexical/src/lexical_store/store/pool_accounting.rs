//! Service-owned inventory and physical pool-file accounting transitions.

use std::fs::File;
use std::os::unix::fs::MetadataExt as _;
use std::path::Path;
use std::sync::Arc;

use quanta_index_core::pool_write_admission::{PoolInventoryEntry, PoolWriteAdmissionPort};
use quanta_index_core::{CoreError, INGEST_RESOURCE_BUDGET_EXCEEDED_CODE};
use rustix::fs::{AtFlags, Mode, OFlags, openat, statat, unlinkat};
use sha2::{Digest as _, Sha256};

use super::{
    MAX_GC_BATCH, MAX_OBJECT_BYTES, ObjectStore, POOL_DIR, atomic_temporary_base, corrupt,
    dir_flags, hex_digest, invalid, is_object_temporary_name, storage,
};

fn entry_name(entry: &rustix::fs::DirEntry) -> Result<String, CoreError> {
    entry
        .file_name()
        .to_str()
        .map(str::to_owned)
        .map_err(|_error| corrupt("non-UTF8 pool inventory entry"))
}

fn parse_digest(name: &str, prefix: u8) -> Result<[u8; 32], CoreError> {
    if name.len() != 64
        || !name
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Err(corrupt("noncanonical pool object name"));
    }
    let mut digest = [0_u8; 32];
    for (slot, pair) in digest.iter_mut().zip(name.as_bytes().chunks_exact(2)) {
        let text = std::str::from_utf8(pair).map_err(|_error| corrupt("pool digest encoding"))?;
        *slot = u8::from_str_radix(text, 16).map_err(|_error| corrupt("pool digest width"))?;
    }
    if digest.first().copied() != Some(prefix) {
        return Err(corrupt("pool object differs from shard prefix"));
    }
    Ok(digest)
}

fn sync_absent_pool_entry(directory: &File, name: &str) -> Result<(), CoreError> {
    match statat(directory, name, AtFlags::SYMLINK_NOFOLLOW) {
        Err(rustix::io::Errno::NOENT) => {}
        Ok(_) => return Err(corrupt("pending pool unlink entry is present")),
        Err(error) => return Err(storage("probe pending pool unlink", error)),
    }
    #[cfg(test)]
    crate::publication_faults::reach(
        crate::publication_faults::Cut::PoolUnlinkRecoveryDirectorySync,
        crate::publication_faults::Side::Before,
    )
    .map_err(|error| storage("sync pending pool unlink", error))?;
    directory
        .sync_all()
        .map_err(|error| storage("sync pending pool unlink", error))
}

impl ObjectStore {
    /// Finish only unlink operations previously observed by this pool's quota.
    /// The caller holds the physical pool gate. Both marker lists are bounded,
    /// and each charge remains until absence and its pinned shard are synced.
    pub(super) fn reconcile_pending_pool_unlinks_locked(&self) -> Result<(), CoreError> {
        let Some(admission) = &self.pool_write_admission else {
            return Ok(());
        };
        let batch_limit = MAX_GC_BATCH
            .checked_div(2)
            .ok_or_else(|| invalid("pending unlink reconciliation batch divisor is zero"))?;
        // Qualified GC retains the actual deletion ticket in SharedGcProgress.
        // A writer must not retire its quota marker independently: the next GC
        // step must first settle that exact ticket, including the observer FD.
        let canonical = if self.object_lifetime.is_some() {
            Vec::new()
        } else {
            admission.pending_canonical_unlinks(batch_limit)?
        };
        let orphans = admission.pending_orphan_unlinks(batch_limit)?;
        if canonical.len() > batch_limit || orphans.len() > batch_limit {
            return Err(corrupt("pending pool unlink list exceeds requested bound"));
        }
        for (digest, bytes) in canonical {
            let directory = self.object_dir(&digest, false)?;
            sync_absent_pool_entry(&directory, &hex_digest(&digest))?;
            admission.confirmed_canonical_unlink(digest, bytes)?;
        }
        for orphan in orphans {
            let base = atomic_temporary_base(&orphan.temp_name)
                .ok_or_else(|| corrupt("pending pool temporary name is invalid"))?;
            let prefix = base
                .get(..2)
                .ok_or_else(|| corrupt("pending pool temporary shard is invalid"))?;
            let prefix = u8::from_str_radix(prefix, 16).map_err(|error| {
                corrupt(&format!("pending pool temporary shard is invalid: {error}"))
            })?;
            if !is_object_temporary_name(&orphan.temp_name, prefix)
                || self.pool_temp_identity(&orphan.temp_name) != orphan.temp_identity
            {
                return Err(corrupt("pending pool temporary identity differs"));
            }
            let digest = parse_digest(base, prefix)?;
            let directory = self.object_dir(&digest, false)?;
            sync_absent_pool_entry(&directory, &orphan.temp_name)?;
            admission.confirmed_orphan_unlink(orphan.temp_identity, orphan.encoded_bytes)?;
        }
        Ok(())
    }

    pub(crate) fn with_pool_write_admission(
        mut self,
        admission: Option<Arc<dyn PoolWriteAdmissionPort>>,
    ) -> Self {
        self.pool_write_admission = admission;
        self
    }

    /// Hashing is infallible by construction over the pinned directory identity and name.
    pub(super) fn pool_temp_identity(&self, name: &str) -> [u8; 32] {
        let mut hash = Sha256::new();
        hash.update(b"lexical-pool-temporary-v1\0");
        hash.update(self.objects_device.to_be_bytes());
        hash.update(self.objects_inode.to_be_bytes());
        hash.update(name.as_bytes());
        hash.finalize().into()
    }

    fn pool_file_width(&self, parent: &File, name: &str) -> Result<u64, CoreError> {
        let file = File::from(
            openat(
                parent,
                name,
                OFlags::RDONLY | OFlags::CLOEXEC | OFlags::NOFOLLOW | OFlags::NONBLOCK,
                Mode::empty(),
            )
            .map_err(|error| storage("open pool inventory file", error))?,
        );
        let metadata = file
            .metadata()
            .map_err(|error| storage("stat pool inventory file", error))?;
        if !metadata.is_file()
            || metadata.dev() != self.objects_device
            || metadata.len()
                > u64::try_from(MAX_OBJECT_BYTES)
                    .map_err(|error| storage("pool object width", error))?
        {
            return Err(corrupt(
                "pool inventory entry is not a bounded regular file",
            ));
        }
        Ok(metadata.len())
    }

    /// Enumerate once under the caller's state-root lease and the pool gate.
    /// Every physical file, including unreachable objects and abandoned temps,
    /// contributes to the boot quota. Payload authentication remains with readers.
    pub(crate) fn visit_existing_pool_inventory(
        state_root: &Path,
        max_files: u64,
        visitor: &mut dyn FnMut(PoolInventoryEntry) -> Result<(), CoreError>,
    ) -> Result<(), CoreError> {
        if max_files == 0 {
            return Err(CoreError::InvalidContract(
                "pool inventory requires a positive file bound".into(),
            ));
        }
        match std::fs::symlink_metadata(state_root.join(POOL_DIR)) {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(error) => return Err(storage("inspect existing pool inventory", error)),
            Ok(metadata) if metadata.is_dir() && !metadata.file_type().is_symlink() => {}
            Ok(_) => return Err(corrupt("pool inventory root is not a real directory")),
        }
        let store = Self::open_existing(state_root)?;
        let _gate = store.lock_gate()?;
        let mut shards = rustix::fs::Dir::read_from(store.objects.as_ref())
            .map_err(|error| storage("enumerate pool inventory shards", error))?;
        let mut inspected = 0_u64;
        while let Some(entry) = shards.read() {
            let entry = entry.map_err(|error| storage("read pool shard entry", error))?;
            let name = entry_name(&entry)?;
            if matches!(name.as_str(), "." | "..") {
                continue;
            }
            if name.len() != 2
                || !name
                    .bytes()
                    .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
            {
                return Err(corrupt("noncanonical pool inventory shard"));
            }
            let prefix = u8::from_str_radix(&name, 16)
                .map_err(|_error| corrupt("pool shard prefix width"))?;
            let shard = File::from(
                openat(
                    store.objects.as_ref(),
                    name.as_str(),
                    dir_flags(),
                    Mode::empty(),
                )
                .map_err(|error| storage("open pool inventory shard", error))?,
            );
            store.check_object_shard_device(&shard)?;
            let mut entries = rustix::fs::Dir::read_from(&shard)
                .map_err(|error| storage("enumerate pool inventory files", error))?;
            while let Some(entry) = entries.read() {
                let entry = entry.map_err(|error| storage("read pool inventory file", error))?;
                let name = entry_name(&entry)?;
                if matches!(name.as_str(), "." | "..") {
                    continue;
                }
                inspected = inspected
                    .checked_add(1)
                    .ok_or_else(|| corrupt("pool inventory file count overflow"))?;
                if inspected > max_files {
                    return Err(CoreError::Typed {
                        code: INGEST_RESOURCE_BUDGET_EXCEEDED_CODE,
                        message: "pool inventory exceeds configured file limit".into(),
                    });
                }
                let bytes = store.pool_file_width(&shard, &name)?;
                if is_object_temporary_name(&name, prefix) {
                    if bytes == 0 {
                        unlinkat(&shard, name.as_str(), AtFlags::empty())
                            .map_err(|error| storage("remove empty crash temporary", error))?;
                        shard.sync_all().map_err(|error| {
                            storage("sync empty crash temporary removal", error)
                        })?;
                    } else {
                        visitor(PoolInventoryEntry::Orphan {
                            temp_identity: store.pool_temp_identity(&name),
                            encoded_bytes: bytes,
                        })?;
                    }
                } else {
                    let digest = parse_digest(&name, prefix)?;
                    if bytes == 0 {
                        return Err(corrupt("empty canonical pool object"));
                    }
                    visitor(PoolInventoryEntry::Canonical {
                        object_sha256: digest,
                        encoded_bytes: bytes,
                    })?;
                }
            }
        }
        Ok(())
    }

    /// The caller holds the pool gate. A partial write retains its original
    /// reservation, which can exceed its actual file length. Only durable removal
    /// releases that reservation; a sync failure conservatively keeps the charge.
    pub(super) fn cleanup_pool_temporary(
        &self,
        parent: &File,
        name: &str,
    ) -> Result<(), CoreError> {
        let _actual_width = self.pool_file_width(parent, name)?;
        let identity = self.pool_temp_identity(name);
        let charge = self
            .pool_write_admission
            .as_ref()
            .map(|admission| {
                admission
                    .orphan_charge(identity)?
                    .ok_or_else(|| corrupt("unaccounted pool temporary during collection"))
            })
            .transpose()?;
        match unlinkat(parent, name, AtFlags::empty()) {
            Ok(()) => {}
            Err(rustix::io::Errno::NOENT) => return Ok(()),
            Err(error) => return Err(storage("remove temporary pool object", error)),
        }
        if let (Some(admission), Some(charge)) = (&self.pool_write_admission, charge) {
            admission.orphan_unlinked_pending_sync(identity, charge, name)?;
        }
        #[cfg(test)]
        crate::publication_faults::reach(
            crate::publication_faults::Cut::PoolOrphanCleanupDirectorySync,
            crate::publication_faults::Side::Before,
        )
        .map_err(|error| storage("sync temporary pool object cleanup", error))?;
        parent
            .sync_all()
            .map_err(|error| storage("sync temporary pool object cleanup", error))?;
        if let (Some(admission), Some(charge)) = (&self.pool_write_admission, charge) {
            admission.confirmed_orphan_unlink(identity, charge)?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests;
