//! Per-install durability choice for an already admitted scalar object.

use std::{fs::File, path::Path};

#[cfg(target_os = "macos")]
use std::{fs, os::unix::fs::MetadataExt as _};

use quanta_index_core::index::IndexError;

#[cfg(target_os = "macos")]
use super::sync_directory_plain_witness;
use super::{
    FileObjectStore, ObjectWriter, SyncKind, SyncedDirectory, storage_error, sync_directory_witness,
};

pub(super) struct ScalarSync {
    #[cfg(target_os = "macos")]
    shared_device: Option<u64>,
}

impl ScalarSync {
    pub(super) fn new(store: &FileObjectStore, shard: &Path) -> Result<Self, IndexError> {
        #[cfg(target_os = "macos")]
        {
            let shared_device = if store.authority.is_some() {
                let objects_device = fs::symlink_metadata(&store.objects)
                    .map_err(|error| storage_error(&error))?
                    .dev();
                let shard_device = fs::symlink_metadata(shard)
                    .map_err(|error| storage_error(&error))?
                    .dev();
                (objects_device == shard_device).then_some(objects_device)
            } else {
                None
            };
            Ok(Self { shared_device })
        }
        #[cfg(not(target_os = "macos"))]
        {
            let _ = (store, shard);
            Ok(Self {})
        }
    }

    pub(super) fn sync_objects(
        &self,
        store: &FileObjectStore,
    ) -> Result<SyncedDirectory, IndexError> {
        store.sync_observed_witness(|| {
            #[cfg(target_os = "macos")]
            if let Some(device) = self.shared_device {
                return sync_directory_plain_witness(&store.objects, device);
            }
            sync_directory_witness(&store.objects)
        })
    }

    pub(super) fn writer_ready(
        &mut self,
        store: &FileObjectStore,
        writer: &ObjectWriter,
        objects_witness: &mut SyncedDirectory,
    ) -> Result<(), IndexError> {
        #[cfg(target_os = "macos")]
        if let Some(device) = self.shared_device
            && writer
                .file()?
                .metadata()
                .map_err(|error| storage_error(&error))?
                .dev()
                != device
        {
            // A mount changed after shard inspection. Restore the original
            // root full sync before the scalar file and shard full syncs.
            *objects_witness =
                store.sync_observed_witness(|| sync_directory_witness(&store.objects))?;
            self.shared_device = None;
        }
        #[cfg(not(target_os = "macos"))]
        let _ = (store, writer, objects_witness);
        Ok(())
    }

    pub(super) fn sync_file(
        &self,
        store: &FileObjectStore,
        writer: &ObjectWriter,
    ) -> Result<(), IndexError> {
        store.sync_observed(SyncKind::File, || {
            #[cfg(target_os = "macos")]
            if self.shared_device.is_some() {
                return rustix::fs::fsync(writer.file()?).map_err(|error| {
                    IndexError::Storage(format!("sync immutable object: {error}"))
                });
            }
            writer
                .file()?
                .sync_all()
                .map_err(|error| storage_error(&error))
        })
    }

    pub(super) fn sync_shard(
        &self,
        store: &FileObjectStore,
        shard: &Path,
    ) -> Result<quanta_index_core::publication_capacity::InodeKey, IndexError> {
        store
            .sync_observed_witness(|| {
                #[cfg(target_os = "macos")]
                if let Some(device) = self.shared_device {
                    return sync_directory_plain_witness(shard, device);
                }
                sync_directory_witness(shard)
            })
            .map(|witness| witness.identity)
    }

    pub(super) fn finish_barrier(
        self,
        store: &FileObjectStore,
        writer: &ObjectWriter,
        barrier: impl FnOnce(&File) -> Result<(), IndexError>,
    ) -> Result<(), IndexError> {
        #[cfg(target_os = "macos")]
        if self.shared_device.is_some() {
            return store.sync_observed(SyncKind::Barrier, || barrier(writer.file()?));
        }
        let _unused = (store, writer, barrier);
        Ok(())
    }
}
