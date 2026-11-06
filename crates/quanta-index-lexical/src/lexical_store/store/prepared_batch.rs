//! Bounded producer buffering before physical pool installation.
//!
//! Prepared bytes are available only to this attempt's scoped builder. They
//! are not retained objects: the pool barrier and every original writer's
//! settlement must complete before an adoption head can name them.

use std::collections::VecDeque;
use std::fs::File;
use std::os::unix::fs::{FileExt as _, MetadataExt as _};
use std::sync::Arc;
use std::sync::atomic::Ordering;

use quanta_index_core::CoreError;
use quanta_index_core::publication_admission::PublicationOutputPermit;
use sha2::{Digest as _, Sha256};

use super::{
    AtomicSync, AttemptLease, BatchState, MAX_ADOPTION_BATCH_DATA_BYTES, MAX_ADOPTION_BATCH_REFS,
    ObjectContents, ObjectRef, ObjectStore, PreparedObject, corrupt, decode_object, invalid,
    storage, with_custody_context,
};

#[derive(Default)]
pub(super) struct PreparedBatch {
    objects: Vec<PreparedObject>,
    bytes: u64,
    closed: bool,
}

impl PreparedBatch {
    pub(super) fn close(&mut self) {
        self.closed = true;
        self.objects = Vec::new();
        self.bytes = 0;
    }
}

pub(super) struct PendingOutput {
    permit: Box<dyn PublicationOutputPermit>,
    writer: Option<File>,
    encoded_bytes: u64,
}

impl PendingOutput {
    pub(super) fn new(
        permit: Box<dyn PublicationOutputPermit>,
        writer: File,
        encoded_bytes: u64,
    ) -> Self {
        Self {
            permit,
            writer: Some(writer),
            encoded_bytes,
        }
    }

    fn commit(&mut self, store: &ObjectStore, expected: Option<&[u8]>) -> Result<(), CoreError> {
        let verification = (|| {
            let expected = expected
                .ok_or_else(|| invalid("pending writer lost its original encoded input"))?;
            store.verify_pool_writer(
                self.writer
                    .as_ref()
                    .ok_or_else(|| invalid("batch writer already retired"))?,
                expected,
            )
        })();
        if let Err(primary) = verification {
            return match self.retire() {
                Ok(()) => Err(primary),
                Err(retirement) => Err(with_custody_context(primary, retirement)),
            };
        }
        let writer = self
            .writer
            .take()
            .ok_or_else(|| invalid("batch writer already retired"))?;
        self.permit.committed(writer, self.encoded_bytes)
    }

    fn retire(&mut self) -> Result<(), CoreError> {
        if let Some(writer) = self.writer.take() {
            self.permit.retire_writer_preserving_pending(writer)?;
        }
        Ok(())
    }
}

impl Drop for PendingOutput {
    fn drop(&mut self) {
        // Normal errors collect retirement failures below. Unwind still
        // consumes the actual FD; the canonical owner quarantines close errors
        // and keeps uncertain capacity instead of treating Drop as proof.
        let _retirement = self.retire();
    }
}

impl ObjectStore {
    pub(super) fn verify_pool_writer(&self, file: &File, expected: &[u8]) -> Result<(), CoreError> {
        if expected.len() > super::MAX_OBJECT_BYTES {
            return Err(invalid("pool writer verification exceeds the object bound"));
        }
        let metadata = file
            .metadata()
            .map_err(|error| storage("stat original pool writer", error))?;
        let expected_len =
            u64::try_from(expected.len()).map_err(|error| storage("pool encoded extent", error))?;
        if !metadata.is_file()
            || metadata.dev() != self.objects_device
            || metadata.len() != expected_len
        {
            return Err(corrupt("original pool writer differs from encoded extent"));
        }
        let mut offset = 0_usize;
        let mut buffer = [0_u8; 8192];
        let result = (|| {
            while offset < expected.len() {
                let remaining = expected
                    .len()
                    .checked_sub(offset)
                    .ok_or_else(|| invalid("pool verification offset exceeds input"))?;
                let width = buffer.len().min(remaining);
                let at =
                    u64::try_from(offset).map_err(|error| storage("pool read offset", error))?;
                let destination = buffer
                    .get_mut(..width)
                    .ok_or_else(|| invalid("pool verification buffer width exceeds bound"))?;
                let read = file
                    .read_at(destination, at)
                    .map_err(|error| storage("read original pool writer", error))?;
                if read == 0 {
                    return Err(corrupt("original pool writer ended before encoded extent"));
                }
                let begin = offset;
                offset = offset
                    .checked_add(read)
                    .ok_or_else(|| invalid("pool verification offset overflow"))?;
                let actual = buffer
                    .get(..read)
                    .ok_or_else(|| invalid("pool verification read exceeds buffer"))?;
                let original = expected
                    .get(begin..offset)
                    .ok_or_else(|| corrupt("original pool writer exceeds encoded extent"))?;
                if actual != original {
                    return Err(corrupt("original pool writer differs from encoded bytes"));
                }
            }
            if file
                .metadata()
                .map_err(|error| storage("recheck original pool extent", error))?
                .len()
                != expected_len
            {
                return Err(corrupt(
                    "original pool writer extent changed during verification",
                ));
            }
            Ok(())
        })();
        let metering = (|| {
            if let Some(meter) = &self.read_meter {
                let bytes =
                    u64::try_from(offset).map_err(|error| storage("pool read width", error))?;
                let _prior = meter
                    .bytes
                    .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |n| {
                        n.checked_add(bytes)
                    })
                    .map_err(|_overflow| invalid("scrub object byte meter overflow"))?;
                let _prior = meter
                    .objects
                    .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |n| n.checked_add(1))
                    .map_err(|_overflow| invalid("scrub object read meter overflow"))?;
            }
            Ok(())
        })();
        match (result, metering) {
            (Ok(()), Ok(())) => Ok(()),
            (Err(primary), Ok(())) | (Ok(()), Err(primary)) => Err(primary),
            (Err(primary), Err(secondary)) => Err(with_custody_context(primary, secondary)),
        }
    }

    pub(super) fn verify_pool_writer_name(
        &self,
        parent: &File,
        name: &str,
        file: &File,
    ) -> Result<(), CoreError> {
        let current = rustix::fs::statat(parent, name, rustix::fs::AtFlags::SYMLINK_NOFOLLOW)
            .map_err(|error| match error {
                rustix::io::Errno::NOENT | rustix::io::Errno::LOOP | rustix::io::Errno::NOTDIR => {
                    corrupt("published pool name is missing or not a regular file")
                }
                other => storage("stat published pool writer", other),
            })?;
        let original = file
            .metadata()
            .map_err(|error| storage("stat original pool identity", error))?;
        if rustix::fs::FileType::from_raw_mode(current.st_mode) != rustix::fs::FileType::RegularFile
            || i128::from(current.st_dev) != i128::from(original.dev())
            || i128::from(current.st_ino) != i128::from(original.ino())
        {
            return Err(corrupt("published pool name differs from original writer"));
        }
        Ok(())
    }
    pub(super) fn require_prepared_batch_open(&self) -> Result<(), CoreError> {
        self.prepared_batch_bytes().map(|_| ())
    }
    pub(super) fn prepared_batch_bytes(&self) -> Result<Option<u64>, CoreError> {
        self.prepared_batch
            .as_ref()
            .map(|batch| {
                let batch = batch
                    .lock()
                    .map_err(|error| storage("lock prepared batch", error))?;
                if batch.closed {
                    return Err(invalid("prepared batch view is closed"));
                }
                Ok(batch.bytes)
            })
            .transpose()
    }

    pub(super) fn stage_prepared_object(
        &self,
        prepared: PreparedObject,
    ) -> Result<(ObjectRef, bool), CoreError> {
        let batch = self
            .prepared_batch
            .as_ref()
            .ok_or_else(|| invalid("prepared batch is absent"))?;
        let mut batch = batch
            .lock()
            .map_err(|error| storage("lock prepared batch", error))?;
        if batch.closed {
            return Err(invalid("prepared batch view is closed"));
        }
        let reference = prepared.reference.clone();
        if batch
            .objects
            .iter()
            .any(|object| object.reference == reference)
        {
            return Ok((reference, false));
        }
        let bytes = batch
            .bytes
            .checked_add(reference.bytes)
            .ok_or_else(|| invalid("prepared batch byte count overflow"))?;
        if batch.objects.len() >= MAX_ADOPTION_BATCH_REFS || bytes > MAX_ADOPTION_BATCH_DATA_BYTES {
            return Err(invalid("prepared objects exceed the adoption batch bounds"));
        }
        batch
            .objects
            .try_reserve(1)
            .map_err(|error| storage("reserve prepared object", error))?;
        batch.objects.push(prepared);
        batch.bytes = bytes;
        drop(batch);
        Ok((reference, true))
    }

    pub(super) fn read_prepared_object(
        &self,
        reference: &ObjectRef,
    ) -> Result<Option<ObjectContents>, CoreError> {
        let Some(batch) = &self.prepared_batch else {
            return Ok(None);
        };
        let batch = batch
            .lock()
            .map_err(|error| storage("lock prepared reader", error))?;
        if batch.closed {
            return Err(invalid("prepared batch view is closed"));
        }
        let Some(prepared) = batch
            .objects
            .iter()
            .find(|object| object.reference.sha256 == reference.sha256)
        else {
            return Ok(None);
        };
        let digest: [u8; 32] = Sha256::digest(&prepared.encoded).into();
        if &prepared.reference != reference || digest != reference.sha256 {
            return Err(corrupt(
                "prepared object differs from its original identity",
            ));
        }
        let (kind, dependencies, payload) = decode_object(&prepared.encoded)?;
        drop(batch);
        if kind != reference.kind {
            return Err(corrupt("prepared object kind differs"));
        }
        Ok(Some((dependencies, payload)))
    }

    pub(super) fn install_prepared_batch(&self, state: &mut BatchState) -> Result<bool, CoreError> {
        let Some(batch) = &self.prepared_batch else {
            return Ok(false);
        };
        let mut batch = batch
            .lock()
            .map_err(|error| storage("lock prepared installation", error))?;
        if batch.closed {
            return Err(invalid("prepared batch view is closed"));
        }
        if batch.objects.is_empty() {
            return Ok(false);
        }
        let mut outputs = VecDeque::new();
        outputs
            .try_reserve_exact(batch.objects.len())
            .map_err(|error| storage("reserve pending batch writers", error))?;
        let lease = AttemptLease {
            store: self.clone(),
            name: state.lease_name.clone(),
            binding: state.binding.clone(),
            write_cost: Arc::clone(&state.write_cost),
        };
        let result = (|| {
            for (index, prepared) in batch.objects.iter().enumerate() {
                let (_, dependencies, _) = decode_object(&prepared.encoded)?;
                let (reference, newly_written, pending) = self.install_prepared_object(
                    prepared,
                    &dependencies,
                    AtomicSync::BatchedPoolData,
                )?;
                if let Some(pending) = pending {
                    outputs.push_back((index, pending));
                }
                lease.record_write_cost(&reference, newly_written)?;
                if newly_written {
                    state.encoded_new_bytes = state
                        .encoded_new_bytes
                        .checked_add(reference.bytes)
                        .ok_or_else(|| invalid("installed batch byte count overflow"))?;
                }
            }
            // POSIX file/shard syncs precede this drive barrier. No output
            // is settled and no adoption head is written before it succeeds.
            self.sync_pool_before_adoption()?;
            while let Some((index, mut output)) = outputs.pop_front() {
                output.commit(
                    self,
                    batch
                        .objects
                        .get(index)
                        .map(|object| object.encoded.as_slice()),
                )?;
                // Linked immutable output remains charged after the permit
                // drops. Release its settled reader pin before the next one.
            }
            Ok(())
        })();
        if let Err(mut primary) = result {
            for (_, output) in &mut outputs {
                if let Err(retirement) = output.retire() {
                    primary = with_custody_context(primary, retirement);
                }
            }
            batch.close();
            return Err(primary);
        }
        batch.objects.clear();
        batch.bytes = 0;
        drop(batch);
        // Every new object was read on its original FD after the barrier and
        // authenticated by its output permit before reaching the journal.
        Ok(true)
    }
}

#[cfg(test)]
mod tests {
    use super::super::{AttemptBinding, BatchScope, OBJECTS_DIR, ObjectKind, POOL_DIR, hex_digest};
    use super::*;
    use std::sync::Mutex;

    const GOLD: &[u8; 24] = b"QLSTOR01\x00\x01\x07\x00\x00\x00\x00\x00\x00\x00\x00\x00\x03abc";

    #[test]
    fn original_writer_verification_detects_same_width_corruption_and_meters_actual_reads() {
        let dir = tempfile::tempdir().expect("root");
        let store = ObjectStore::open(dir.path())
            .expect("store")
            .metered_clone();
        let mut file = tempfile::tempfile_in(dir.path()).expect("writer");
        std::io::Write::write_all(&mut file, GOLD).expect("write");
        store
            .verify_pool_writer(&file, GOLD)
            .expect("exact persisted bytes");
        assert_eq!(store.read_meter_snapshot(), (24, 1));
        std::os::unix::fs::FileExt::write_all_at(&file, b"X", 23).expect("same-width corruption");
        assert!(matches!(
            store.verify_pool_writer(&file, GOLD),
            Err(CoreError::Typed {
                code: quanta_index_contract::SearchPlaneErrorCodeV2::GenerationSidecarCorrupt,
                ..
            })
        ));
        assert_eq!(store.read_meter_snapshot(), (48, 2));
    }

    #[test]
    fn original_writer_verification_rejects_a_foreign_published_name() {
        let dir = tempfile::tempdir().expect("root");
        let store = ObjectStore::open(dir.path()).expect("store");
        let target = dir.path().join("original");
        let mut file = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create_new(true)
            .open(&target)
            .expect("writer");
        std::io::Write::write_all(&mut file, GOLD).expect("write");
        let parent = File::open(dir.path()).expect("parent");
        store
            .verify_pool_writer_name(&parent, "original", &file)
            .expect("original name");
        std::fs::rename(&target, dir.path().join("saved")).expect("move original");
        std::fs::write(&target, GOLD).expect("same bytes on a foreign inode");
        assert!(matches!(
            store.verify_pool_writer_name(&parent, "original", &file),
            Err(CoreError::Typed {
                code: quanta_index_contract::SearchPlaneErrorCodeV2::GenerationSidecarCorrupt,
                ..
            })
        ));
        std::fs::remove_file(&target).expect("remove foreign name");
        assert!(matches!(
            store.verify_pool_writer_name(&parent, "original", &file),
            Err(CoreError::Typed {
                code: quanta_index_contract::SearchPlaneErrorCodeV2::GenerationSidecarCorrupt,
                ..
            })
        ));
    }

    fn buffered(store: &ObjectStore, lease: &AttemptLease) -> ObjectStore {
        let mut scoped = store.clone();
        scoped.prepared_batch = Some(Arc::new(Mutex::new(PreparedBatch::default())));
        scoped.adoption_batch = Some(Arc::new(Mutex::new(BatchState {
            lease_name: lease.name.clone(),
            binding: lease.binding.clone(),
            write_cost: Arc::clone(&lease.write_cost),
            gate: None,
            references: Vec::new(),
            encoded_new_bytes: 0,
            closed: false,
        })));
        scoped
    }

    fn binding() -> AttemptBinding {
        AttemptBinding {
            root_incarnation: [1; 16],
            repository_sha256: [2; 32],
            revision_sha256: [3; 32],
            source_event_sha256: [4; 32],
            target_generation: 1,
            base_root_sha256: None,
            operations_sha256: [5; 32],
            format: 1,
        }
    }

    #[test]
    fn prepared_bytes_are_private_until_barrier_and_adoption() {
        let dir = tempfile::tempdir().expect("root");
        let store = ObjectStore::open(dir.path()).expect("store");
        let lease = store.begin_attempt(&binding(), &[]).expect("attempt");
        let scoped = buffered(&store, &lease);
        let _scope = BatchScope { store: &scoped };
        let reference = scoped
            .put_adopted_object(&lease, ObjectKind::Record, &[], b"abc")
            .expect("prepare object");
        assert_eq!(reference.bytes, 24);
        let expected_digest: [u8; 32] = Sha256::digest(GOLD).into();
        assert_eq!(reference.sha256, expected_digest);
        assert_eq!(
            scoped
                .read_object(&reference, None)
                .expect("producer read")
                .1,
            b"abc"
        );
        assert!(
            store.read_object(&reference, None).is_err(),
            "unscoped read must not see staging"
        );
        assert!(store.adopted_roots().expect("roots").is_empty());
        assert_eq!(lease.write_cost_snapshot().expect("cost").new_objects, 0);
        scoped
            .adopt_existing_root(&lease, &reference)
            .expect("private dependency");
        scoped.flush_adoption_batch().expect("install and adopt");
        assert!(
            store
                .adopted_roots()
                .expect("durable roots")
                .contains(&reference)
        );
        let digest = hex_digest(&reference.sha256);
        let path = dir
            .path()
            .join(POOL_DIR)
            .join(OBJECTS_DIR)
            .join(digest.get(..2).expect("shard"))
            .join(digest);
        assert_eq!(std::fs::read(path).expect("physical bytes"), GOLD);
        assert_eq!(scoped.prepared_batch_bytes().expect("buffer"), Some(0));
    }

    #[test]
    fn failed_prepared_barrier_never_advances_head_or_leaves_a_readable_scope() {
        use crate::publication_faults::{Cut, Side, with_error_at};
        let dir = tempfile::tempdir().expect("root");
        let store = ObjectStore::open(dir.path()).expect("store");
        let active = binding();
        let lease = store.begin_attempt(&active, &[]).expect("attempt");
        let scoped = buffered(&store, &lease);
        let scope = BatchScope { store: &scoped };
        let reference = scoped
            .put_adopted_object(&lease, ObjectKind::Record, &[], b"abc")
            .expect("prepare object");
        let (failed, fired) = with_error_at(Cut::PoolFullBarrier, Side::Before, 1, || {
            scoped.flush_adoption_batch()
        });
        assert!(fired);
        assert!(failed.is_err());
        assert_eq!(
            store
                .read_journal_head(&active.identity_name())
                .expect("head")
                .committed_length,
            0
        );
        assert!(store.adopted_roots().expect("roots").is_empty());
        assert!(matches!(
            scoped.read_object(&reference, None),
            Err(CoreError::InvalidContract(_))
        ));
        drop(scope);
        drop(scoped);
        drop(lease);
        let reopened = ObjectStore::open(dir.path()).expect("cold open");
        assert!(reopened.adopted_roots().expect("cold roots").is_empty());
        let retry = reopened.begin_attempt(&active, &[]).expect("retry");
        let retried = reopened
            .put_adopted_object(&retry, ObjectKind::Record, &[], b"abc")
            .expect("repair and adopt");
        assert_eq!(retried, reference);
    }

    #[cfg(target_vendor = "apple")]
    #[test]
    fn new_shard_sync_failure_never_adopts_and_cold_retry_repairs_parent_metadata() {
        use crate::publication_faults::{Cut, Side, with_error_at};
        for cut in [Cut::PoolNewDirectorySyncCall, Cut::PoolNewParentSyncCall] {
            let dir = tempfile::tempdir().expect("root");
            let store = ObjectStore::open(dir.path()).expect("store");
            let active = binding();
            let lease = store.begin_attempt(&active, &[]).expect("attempt");
            let scoped = buffered(&store, &lease);
            let scope = BatchScope { store: &scoped };
            let reference = scoped
                .put_adopted_object(&lease, ObjectKind::Record, &[], b"abc")
                .expect("prepare");
            let (failed, fired) =
                with_error_at(cut, Side::Before, 1, || scoped.flush_adoption_batch());
            assert!(fired);
            assert!(failed.is_err());
            assert_eq!(
                lease.write_cost_snapshot().expect("no birth").new_objects,
                0
            );
            assert_eq!(
                store
                    .read_journal_head(&active.identity_name())
                    .expect("head")
                    .committed_length,
                0
            );
            assert!(store.adopted_roots().expect("roots").is_empty());
            drop(scope);
            drop(scoped);
            drop(lease);
            drop(store);
            let cold = ObjectStore::open(dir.path()).expect("cold store");
            let retry = cold.begin_attempt(&active, &[]).expect("retry");
            let next = buffered(&cold, &retry);
            let scope = BatchScope { store: &next };
            let retried = next
                .put_adopted_object(&retry, ObjectKind::Record, &[], b"abc")
                .expect("prepare retry");
            assert_eq!(reference, retried);
            // An existing shard from the failed mkdir path must still cross
            // its parent-sync boundary before any new file can be adopted.
            let (failed, fired) =
                with_error_at(Cut::PoolNewParentSyncCall, Side::Before, 1, || {
                    next.flush_adoption_batch()
                });
            assert!(fired);
            assert!(failed.is_err());
            assert_eq!(
                retry
                    .write_cost_snapshot()
                    .expect("still no birth")
                    .new_objects,
                0
            );
            drop(scope);
            drop(next);
            let committed = cold
                .put_adopted_object(&retry, ObjectKind::Record, &[], b"abc")
                .expect("repair and commit");
            assert_eq!(reference, committed);
            assert_eq!(
                cold.read_object(&committed, None).expect("stored").1,
                b"abc"
            );
        }
    }

    #[test]
    fn prepared_count_and_byte_caps_precede_physical_creation() {
        let dir = tempfile::tempdir().expect("root");
        let store = ObjectStore::open(dir.path()).expect("store");
        let lease = store.begin_attempt(&binding(), &[]).expect("attempt");
        let scoped = buffered(&store, &lease);
        let _scope = BatchScope { store: &scoped };
        for value in 0..128_u16 {
            let _object = scoped
                .stage_prepared_object(
                    PreparedObject::new(ObjectKind::Record, &[], &value.to_be_bytes())
                        .expect("object"),
                )
                .expect("bounded preparation");
        }
        assert!(
            scoped
                .stage_prepared_object(
                    PreparedObject::new(ObjectKind::Record, &[], b"overflow").expect("object")
                )
                .is_err()
        );
        assert!(store.adopted_roots().expect("roots").is_empty());
        assert_eq!(lease.write_cost_snapshot().expect("cost").new_objects, 0);
        let byte_limited = buffered(&store, &lease);
        let _byte_scope = BatchScope {
            store: &byte_limited,
        };
        for value in 0..31_u8 {
            let payload = vec![value; 65_536];
            let _object = byte_limited
                .stage_prepared_object(
                    PreparedObject::new(ObjectKind::Record, &[], &payload).expect("object"),
                )
                .expect("bounded bytes");
        }
        assert!(
            byte_limited
                .stage_prepared_object(
                    PreparedObject::new(ObjectKind::Record, &[], &vec![31; 65_536])
                        .expect("object")
                )
                .is_err()
        );
        assert_eq!(
            byte_limited.prepared_batch_bytes().expect("bytes"),
            Some(2_032_267)
        );
    }

    #[test]
    fn escaped_producer_cannot_read_after_scope_retirement() {
        let dir = tempfile::tempdir().expect("root");
        let store = ObjectStore::open(dir.path()).expect("store");
        let lease = store.begin_attempt(&binding(), &[]).expect("attempt");
        let scoped = buffered(&store, &lease);
        let scope = BatchScope { store: &scoped };
        let escaped = scoped.metered_clone();
        let reference = escaped
            .put_adopted_object(&lease, ObjectKind::Record, &[], b"abc")
            .expect("prepare");
        let _read = escaped
            .read_object(&reference, None)
            .expect("producer read");
        scoped.flush_adoption_batch().expect("install");
        drop(scope);
        assert!(store.read_object(&reference, None).is_ok());
        assert!(matches!(
            escaped.read_object(&reference, None),
            Err(CoreError::InvalidContract(_))
        ));
    }
}
