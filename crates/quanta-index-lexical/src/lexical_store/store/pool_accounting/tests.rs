use std::collections::BTreeMap;
use std::fs;
use std::os::unix::fs::symlink;
use std::sync::{Arc, Mutex};

use quanta_index_core::CoreError;
use quanta_index_core::pool_write_admission::{
    PendingOrphanUnlink, PoolInventoryEntry, PoolWriteAdmissionPort, PoolWritePermit,
    PoolWriteUsage,
};

use super::*;
use crate::lexical_store::{AttemptBinding, ObjectKind};

fn binding() -> AttemptBinding {
    AttemptBinding {
        root_incarnation: [1; 16],
        repository_sha256: [2; 32],
        revision_sha256: [5; 32],
        source_event_sha256: [3; 32],
        target_generation: 1,
        base_root_sha256: None,
        operations_sha256: [4; 32],
        format: 1,
    }
}

fn object_dir(root: &Path, digest: [u8; 32]) -> std::path::PathBuf {
    root.join(POOL_DIR)
        .join("objects")
        .join(format!("{:02x}", digest.first().expect("digest prefix")))
}

fn canonical_path(root: &Path, digest: [u8; 32]) -> std::path::PathBuf {
    object_dir(root, digest).join(super::super::hex_digest(&digest))
}

fn write_unreachable(root: &Path, digest: [u8; 32], bytes: &[u8]) {
    let directory = object_dir(root, digest);
    fs::create_dir_all(&directory).expect("shard");
    fs::write(canonical_path(root, digest), bytes).expect("unreachable canonical");
}

#[test]
fn boot_inventory_counts_unreachable_canonical_and_orphan_and_removes_empty_temp() {
    let dir = tempfile::tempdir().expect("state root");
    let store = ObjectStore::open(dir.path()).expect("pool bootstrap");
    let digest = [0x0a; 32];
    write_unreachable(dir.path(), digest, b"unreachable");
    let shard = object_dir(dir.path(), digest);
    let name = super::super::hex_digest(&digest);
    let orphan = format!(".{name}.tmp-123-1");
    let empty = format!(".{name}.tmp-123-2");
    fs::write(shard.join(&orphan), b"partial").expect("orphan");
    fs::write(shard.join(&empty), []).expect("empty temp");

    let mut rows = Vec::new();
    ObjectStore::visit_existing_pool_inventory(dir.path(), 3, &mut |row| {
        rows.push(row);
        Ok(())
    })
    .expect("inventory");
    assert!(rows.contains(&PoolInventoryEntry::Canonical {
        object_sha256: digest,
        encoded_bytes: 11,
    }));
    assert!(rows.contains(&PoolInventoryEntry::Orphan {
        temp_identity: store.pool_temp_identity(&orphan),
        encoded_bytes: 7,
    }));
    assert_eq!(rows.len(), 2);
    assert!(!shard.join(empty).exists());
}

#[test]
fn boot_inventory_refuses_noncanonical_symlink_oversize_and_file_limit() {
    let dir = tempfile::tempdir().expect("state root");
    let _store = ObjectStore::open(dir.path()).expect("pool bootstrap");
    write_unreachable(dir.path(), [0x0a; 32], b"a");
    write_unreachable(dir.path(), [0x0b; 32], b"b");
    assert!(matches!(
        ObjectStore::visit_existing_pool_inventory(dir.path(), 1, &mut |_row| Ok(())),
        Err(CoreError::Typed {
            code: INGEST_RESOURCE_BUDGET_EXCEEDED_CODE,
            ..
        })
    ));

    let dir = tempfile::tempdir().expect("state root");
    let _store = ObjectStore::open(dir.path()).expect("pool bootstrap");
    fs::create_dir_all(dir.path().join(POOL_DIR).join("objects").join("ZZ"))
        .expect("noncanonical shard");
    assert!(ObjectStore::visit_existing_pool_inventory(dir.path(), 4, &mut |_row| Ok(())).is_err());

    let dir = tempfile::tempdir().expect("state root");
    let _store = ObjectStore::open(dir.path()).expect("pool bootstrap");
    let digest = [0x0a; 32];
    let shard = object_dir(dir.path(), digest);
    fs::create_dir_all(&shard).expect("shard");
    symlink(
        dir.path().join(POOL_DIR),
        canonical_path(dir.path(), digest),
    )
    .expect("symlink entry");
    assert!(ObjectStore::visit_existing_pool_inventory(dir.path(), 4, &mut |_row| Ok(())).is_err());

    let dir = tempfile::tempdir().expect("state root");
    let _store = ObjectStore::open(dir.path()).expect("pool bootstrap");
    write_unreachable(dir.path(), digest, b"x");
    fs::OpenOptions::new()
        .write(true)
        .open(canonical_path(dir.path(), digest))
        .expect("sparse file")
        .set_len(
            u64::try_from(MAX_OBJECT_BYTES)
                .expect("max width")
                .checked_add(1)
                .expect("oversize width"),
        )
        .expect("oversize sparse file");
    assert!(ObjectStore::visit_existing_pool_inventory(dir.path(), 4, &mut |_row| Ok(())).is_err());
}

#[derive(Default)]
struct ObservedCalls {
    reserved: Vec<([u8; 32], u64)>,
    pending_temps: BTreeMap<[u8; 32], u64>,
    orphans: BTreeMap<[u8; 32], u64>,
    canonical: BTreeMap<[u8; 32], u64>,
    canonical_releases: Vec<([u8; 32], u64)>,
    pending_canonical_unlinks: BTreeMap<[u8; 32], u64>,
    pending_orphan_unlinks: BTreeMap<[u8; 32], PendingOrphanUnlink>,
    orphan_releases: Vec<([u8; 32], u64)>,
    reused: usize,
}

#[derive(Default)]
struct RecordingAdmission(Arc<Mutex<ObservedCalls>>);

struct RecordingPermit {
    calls: Arc<Mutex<ObservedCalls>>,
    object: [u8; 32],
    bytes: u64,
    temp: Option<[u8; 32]>,
    existing: bool,
    done: bool,
}

fn record_error(message: &str) -> CoreError {
    CoreError::InvalidContract(message.into())
}

impl PoolWriteAdmissionPort for RecordingAdmission {
    fn try_reserve(
        &self,
        object_sha256: [u8; 32],
        encoded_bytes: u64,
    ) -> Result<Box<dyn PoolWritePermit>, CoreError> {
        let mut calls = self.0.lock().expect("recording lock");
        calls.reserved.push((object_sha256, encoded_bytes));
        let existing = calls.canonical.contains_key(&object_sha256);
        drop(calls);
        Ok(Box::new(RecordingPermit {
            calls: Arc::clone(&self.0),
            object: object_sha256,
            bytes: encoded_bytes,
            temp: None,
            existing,
            done: false,
        }))
    }

    fn canonical_unlinked_pending_sync(
        &self,
        object_sha256: [u8; 32],
        encoded_bytes: u64,
    ) -> Result<(), CoreError> {
        let mut calls = self.0.lock().expect("quota observation lock");
        if calls.canonical.get(&object_sha256) != Some(&encoded_bytes)
            || calls.pending_canonical_unlinks.contains_key(&object_sha256)
        {
            return Err(record_error("pending canonical release mismatch"));
        }
        let _previous = calls
            .pending_canonical_unlinks
            .insert(object_sha256, encoded_bytes);
        drop(calls);
        Ok(())
    }

    fn pending_canonical_unlinks(
        &self,
        max_objects: usize,
    ) -> Result<Vec<([u8; 32], u64)>, CoreError> {
        Ok(self
            .0
            .lock()
            .expect("quota observation lock")
            .pending_canonical_unlinks
            .iter()
            .take(max_objects)
            .map(|(identity, bytes)| (*identity, *bytes))
            .collect())
    }

    fn orphan_unlinked_pending_sync(
        &self,
        temp_identity: [u8; 32],
        encoded_bytes: u64,
        temp_name: &str,
    ) -> Result<(), CoreError> {
        let mut calls = self.0.lock().expect("quota observation lock");
        if calls.orphans.get(&temp_identity) != Some(&encoded_bytes)
            || calls.pending_orphan_unlinks.contains_key(&temp_identity)
        {
            return Err(record_error("pending orphan release mismatch"));
        }
        let _previous = calls.pending_orphan_unlinks.insert(
            temp_identity,
            PendingOrphanUnlink {
                temp_identity,
                encoded_bytes,
                temp_name: temp_name.to_owned(),
            },
        );
        drop(calls);
        Ok(())
    }

    fn pending_orphan_unlinks(
        &self,
        max_objects: usize,
    ) -> Result<Vec<PendingOrphanUnlink>, CoreError> {
        Ok(self
            .0
            .lock()
            .expect("quota observation lock")
            .pending_orphan_unlinks
            .values()
            .take(max_objects)
            .cloned()
            .collect())
    }

    fn confirmed_canonical_unlink(
        &self,
        object_sha256: [u8; 32],
        encoded_bytes: u64,
    ) -> Result<(), CoreError> {
        let mut calls = self.0.lock().expect("recording lock");
        if calls.canonical.remove(&object_sha256) != Some(encoded_bytes) {
            return Err(record_error("canonical release mismatch"));
        }
        calls
            .canonical_releases
            .push((object_sha256, encoded_bytes));
        let _pending = calls.pending_canonical_unlinks.remove(&object_sha256);
        drop(calls);
        Ok(())
    }

    fn confirmed_orphan_unlink(
        &self,
        temp_identity: [u8; 32],
        encoded_bytes: u64,
    ) -> Result<(), CoreError> {
        let mut calls = self.0.lock().expect("recording lock");
        if calls.orphans.remove(&temp_identity) != Some(encoded_bytes) {
            return Err(record_error("orphan release mismatch"));
        }
        calls.orphan_releases.push((temp_identity, encoded_bytes));
        let _pending = calls.pending_orphan_unlinks.remove(&temp_identity);
        drop(calls);
        Ok(())
    }

    fn orphan_charge(&self, temp_identity: [u8; 32]) -> Result<Option<u64>, CoreError> {
        Ok(self
            .0
            .lock()
            .expect("recording lock")
            .orphans
            .get(&temp_identity)
            .copied())
    }

    fn snapshot(&self) -> Result<PoolWriteUsage, CoreError> {
        let calls = self.0.lock().expect("recording lock");
        let canonical_bytes: u64 = calls.canonical.values().sum();
        let orphan_bytes: u64 = calls.orphans.values().sum();
        let in_flight_bytes: u64 = calls.pending_temps.values().sum();
        Ok(PoolWriteUsage {
            file_limit: u64::MAX,
            charged_files: u64::try_from(calls.canonical.len())
                .expect("canonical count")
                .checked_add(u64::try_from(calls.orphans.len()).expect("orphan count"))
                .and_then(|count| {
                    count.checked_add(
                        u64::try_from(calls.pending_temps.len()).expect("pending count"),
                    )
                })
                .expect("file count"),
            ceiling_bytes: u64::MAX,
            canonical_bytes,
            in_flight_bytes,
            orphan_bytes,
            total_charged_bytes: canonical_bytes
                .checked_add(orphan_bytes)
                .and_then(|bytes| bytes.checked_add(in_flight_bytes))
                .expect("charged bytes"),
        })
    }
}

impl PoolWritePermit for RecordingPermit {
    fn check_new_file_admitted(&self) -> Result<(), CoreError> {
        if self.existing || self.done {
            return Err(record_error("reuse permit cannot create a new file"));
        }
        Ok(())
    }

    fn temp_created(&mut self, temp_identity: [u8; 32]) -> Result<(), CoreError> {
        self.check_new_file_admitted()?;
        if self.temp.replace(temp_identity).is_some() {
            return Err(record_error("duplicate temporary callback"));
        }
        let _previous = self
            .calls
            .lock()
            .expect("recording lock")
            .pending_temps
            .insert(temp_identity, self.bytes);
        Ok(())
    }

    fn canonical_renamed(&mut self) -> Result<(), CoreError> {
        let temp = self
            .temp
            .take()
            .ok_or_else(|| record_error("temp absent"))?;
        let mut calls = self.calls.lock().expect("recording lock");
        if calls.pending_temps.remove(&temp) != Some(self.bytes) {
            return Err(record_error("temp charge absent"));
        }
        let _previous = calls.canonical.insert(self.object, self.bytes);
        drop(calls);
        self.done = true;
        Ok(())
    }

    fn temp_unlinked_and_synced(&mut self) -> Result<(), CoreError> {
        let temp = self
            .temp
            .take()
            .ok_or_else(|| record_error("temp absent"))?;
        if self
            .calls
            .lock()
            .expect("recording lock")
            .pending_temps
            .remove(&temp)
            != Some(self.bytes)
        {
            return Err(record_error("temp charge absent"));
        }
        self.done = true;
        Ok(())
    }

    fn reused_verified(&mut self) -> Result<(), CoreError> {
        if !self.existing || self.temp.is_some() || self.done {
            return Err(record_error("unexpected reuse callback"));
        }
        let mut calls = self.calls.lock().expect("recording lock");
        calls.reused = calls.reused.checked_add(1).expect("reuse count headroom");
        drop(calls);
        self.done = true;
        Ok(())
    }
}

impl Drop for RecordingPermit {
    fn drop(&mut self) {
        if self.done {
            return;
        }
        if let Some(temp) = self.temp {
            let mut calls = self.calls.lock().expect("recording lock");
            let charge = calls.pending_temps.remove(&temp).expect("pending temp");
            let _previous = calls.orphans.insert(temp, charge);
        }
    }
}

#[test]
fn short_write_cleanup_refunds_original_charge_only_after_real_temp_removal() {
    use crate::publication_faults::with_pool_partial_write_at;

    let dir = tempfile::tempdir().expect("state root");
    let admission = Arc::new(RecordingAdmission::default());
    let store = ObjectStore::open(dir.path())
        .expect("pool bootstrap")
        .with_pool_write_admission(Some(admission.clone()));
    let lease = store.begin_attempt(&binding(), &[]).expect("attempt");
    let (result, fired) = with_pool_partial_write_at(3, 1, || {
        store.put_blob(&lease, ObjectKind::Record, &[], b"record payload")
    });
    assert!(fired);
    assert!(result.is_err());
    let temp = {
        let calls = admission.0.lock().expect("recording lock");
        assert_eq!(calls.reserved.len(), 1);
        assert_eq!(calls.orphans.len(), 1);
        assert!(calls.orphan_releases.is_empty());
        *calls.orphans.keys().next().expect("orphan identity")
    };
    let actual_temp = fs::read_dir(dir.path().join(POOL_DIR).join("objects"))
        .expect("shards")
        .flat_map(|shard| fs::read_dir(shard.expect("shard").path()).expect("shard entries"))
        .map(|entry| entry.expect("entry").path())
        .find(|path| {
            path.file_name()
                .expect("name")
                .to_string_lossy()
                .starts_with('.')
        })
        .expect("partial temporary");
    assert_eq!(fs::metadata(&actual_temp).expect("temp metadata").len(), 3);
    let expected_charge = admission
        .0
        .lock()
        .expect("recording lock")
        .reserved
        .first()
        .expect("one reservation")
        .1;
    assert!(expected_charge > 3);
    store
        .visit_object_digests(&mut |_digest| Ok(()))
        .expect("GC temp cleanup");
    assert!(!actual_temp.exists());
    let calls = admission.0.lock().expect("recording lock");
    assert_eq!(calls.orphan_releases, vec![(temp, expected_charge)]);
    assert!(calls.orphans.is_empty());
    drop(calls);
}

#[test]
fn corrupted_canonical_reuse_does_not_report_reuse_or_refund() {
    let dir = tempfile::tempdir().expect("state root");
    let admission = Arc::new(RecordingAdmission::default());
    let store = ObjectStore::open(dir.path())
        .expect("pool bootstrap")
        .with_pool_write_admission(Some(admission.clone()));
    let lease = store.begin_attempt(&binding(), &[]).expect("attempt");
    let value = store
        .put_blob(&lease, ObjectKind::Record, &[], b"record payload")
        .expect("first canonical");
    let canonical = canonical_path(dir.path(), value.root.sha256);
    assert!(canonical.exists());
    fs::write(&canonical, b"bad").expect("corrupt canonical bytes");
    assert!(
        store
            .put_blob(&lease, ObjectKind::Record, &[], b"record payload")
            .is_err()
    );
    let calls = admission.0.lock().expect("recording lock");
    assert_eq!(calls.reserved.len(), 2);
    assert_eq!(calls.reused, 0);
    assert!(calls.canonical.contains_key(&value.root.sha256));
    assert!(calls.orphan_releases.is_empty());
    drop(calls);
}

#[test]
fn gc_sync_failure_retains_charge_until_same_digest_write_finishes_durable_unlink() {
    use crate::publication_faults::{Cut, Side, with_error_at};

    let dir = tempfile::tempdir().expect("state root");
    let admission = Arc::new(RecordingAdmission::default());
    let store = ObjectStore::open(dir.path())
        .expect("pool bootstrap")
        .with_pool_write_admission(Some(admission.clone()));
    let value = {
        let _gate = store.lock_gate().expect("pool gate");
        store
            .put_object_locked(ObjectKind::Record, &[], b"collectable record")
            .expect("physical object")
    };
    let path = canonical_path(dir.path(), value.sha256);
    let encoded = fs::read(&path).expect("independent physical bytes");
    assert_eq!(
        u64::try_from(encoded.len()).expect("file width"),
        value.bytes
    );
    let epoch = store.read_epoch().expect("epoch");
    let (result, fired) = with_error_at(Cut::PoolGcDirectorySync, Side::Before, 1, || {
        store.gc_sweep_batch(
            epoch,
            &[value.sha256],
            &std::collections::BTreeSet::default(),
        )
    });
    assert!(fired);
    assert!(matches!(result, Err(CoreError::Storage(_))));
    assert!(!path.exists(), "unlink actually preceded the failed sync");
    {
        let calls = admission.0.lock().expect("recording lock");
        assert_eq!(calls.canonical.get(&value.sha256), Some(&value.bytes));
        assert_eq!(
            calls.pending_canonical_unlinks.get(&value.sha256),
            Some(&value.bytes)
        );
        assert!(calls.canonical_releases.is_empty());
        drop(calls);
    }
    let (result, fired) = with_error_at(
        Cut::PoolUnlinkRecoveryDirectorySync,
        Side::Before,
        1,
        || {
            let _gate = store.lock_gate().expect("pool gate");
            store.put_object_locked(ObjectKind::Record, &[], b"collectable record")
        },
    );
    assert!(fired);
    assert!(matches!(result, Err(CoreError::Storage(_))));
    assert!(!path.exists());
    {
        let calls = admission.0.lock().expect("recording lock");
        assert_eq!(
            calls.reserved.len(),
            1,
            "no new permit before durable recovery"
        );
        assert!(calls.canonical_releases.is_empty());
        assert_eq!(calls.canonical.get(&value.sha256), Some(&value.bytes));
        drop(calls);
    }
    let replacement = {
        let _gate = store.lock_gate().expect("pool gate");
        store
            .put_object_locked(ObjectKind::Record, &[], b"collectable record")
            .expect("same digest write after durable recovery")
    };
    assert_eq!(replacement, value);
    assert_eq!(fs::read(path).expect("recreated canonical"), encoded);
    let calls = admission.0.lock().expect("recording lock");
    assert_eq!(calls.canonical_releases, vec![(value.sha256, value.bytes)]);
    assert!(calls.pending_canonical_unlinks.is_empty());
    assert_eq!(calls.canonical.get(&value.sha256), Some(&value.bytes));
    assert_eq!(calls.reserved.len(), 2);
    assert_eq!(
        calls.reused, 0,
        "the missing object required a fresh permit"
    );
    drop(calls);
}

#[test]
fn fresh_gc_inventory_recovers_pending_unlink_without_a_physical_candidate() {
    use crate::publication_faults::{Cut, Side, with_error_at};

    let dir = tempfile::tempdir().expect("state root");
    let admission = Arc::new(RecordingAdmission::default());
    let store = ObjectStore::open(dir.path())
        .expect("pool bootstrap")
        .with_pool_write_admission(Some(admission.clone()));
    let value = {
        let _gate = store.lock_gate().expect("pool gate");
        store
            .put_object_locked(ObjectKind::Record, &[], b"unreachable record")
            .expect("physical object")
    };
    let epoch = store.read_epoch().expect("epoch");
    let (result, fired) = with_error_at(Cut::PoolGcDirectorySync, Side::Before, 1, || {
        store.gc_sweep_batch(
            epoch,
            &[value.sha256],
            &std::collections::BTreeSet::default(),
        )
    });
    assert!(fired && result.is_err());
    assert!(!canonical_path(dir.path(), value.sha256).exists());
    let _cursor = store
        .object_inventory_cursor()
        .expect("fresh production inventory");
    assert_eq!(admission.snapshot().expect("usage").total_charged_bytes, 0);
    let calls = admission.0.lock().expect("recording lock");
    assert!(calls.pending_canonical_unlinks.is_empty());
    assert_eq!(calls.canonical_releases, vec![(value.sha256, value.bytes)]);
    drop(calls);
}

#[test]
fn orphan_cleanup_sync_failure_recovers_original_reservation_after_name_disappears() {
    use crate::publication_faults::{Cut, Side, with_error_at, with_pool_partial_write_at};

    let dir = tempfile::tempdir().expect("state root");
    let admission = Arc::new(RecordingAdmission::default());
    let store = ObjectStore::open(dir.path())
        .expect("pool bootstrap")
        .with_pool_write_admission(Some(admission.clone()));
    let lease = store.begin_attempt(&binding(), &[]).expect("attempt");
    let (result, fired) = with_pool_partial_write_at(3, 1, || {
        store.put_blob(&lease, ObjectKind::Record, &[], b"interrupted record")
    });
    assert!(fired && result.is_err());
    let (identity, charge) = {
        let calls = admission.0.lock().expect("recording lock");
        assert_eq!(calls.orphans.len(), 1);
        let (identity, charge) = calls.orphans.iter().next().expect("orphan");
        assert!(
            *charge > 3,
            "the original reservation exceeds the partial file"
        );
        let pair = (*identity, *charge);
        drop(calls);
        pair
    };
    let (result, fired) =
        with_error_at(Cut::PoolOrphanCleanupDirectorySync, Side::Before, 1, || {
            store.visit_object_digests(&mut |_digest| Ok(()))
        });
    assert!(fired);
    assert!(matches!(result, Err(CoreError::Storage(_))));
    let pending = admission.pending_orphan_unlinks(1).expect("pending unlink");
    assert_eq!(pending.len(), 1);
    let marker = pending.first().expect("one marker");
    assert_eq!(marker.temp_identity, identity);
    assert_eq!(marker.encoded_bytes, charge);
    let base = super::super::atomic_temporary_base(&marker.temp_name).expect("temp base");
    let prefix = u8::from_str_radix(base.get(..2).expect("prefix"), 16).expect("hex prefix");
    let digest = parse_digest(base, prefix).expect("digest");
    assert!(
        !object_dir(dir.path(), digest)
            .join(&marker.temp_name)
            .exists()
    );
    assert_eq!(
        admission.snapshot().expect("retained charge").orphan_bytes,
        charge
    );
    let (result, fired) = with_error_at(
        Cut::PoolUnlinkRecoveryDirectorySync,
        Side::Before,
        1,
        || store.object_inventory_cursor(),
    );
    assert!(fired && result.is_err());
    assert_eq!(
        admission.snapshot().expect("still charged").orphan_bytes,
        charge
    );
    assert!(
        admission
            .0
            .lock()
            .expect("recording lock")
            .orphan_releases
            .is_empty()
    );
    let _cursor = store
        .object_inventory_cursor()
        .expect("retry production inventory");
    assert_eq!(
        admission
            .snapshot()
            .expect("recovered usage")
            .total_charged_bytes,
        0
    );
    let calls = admission.0.lock().expect("recording lock");
    assert!(calls.pending_orphan_unlinks.is_empty());
    assert_eq!(calls.orphan_releases, vec![(identity, charge)]);
    drop(calls);
}

#[test]
fn pending_unlink_that_reappears_is_rejected_without_refund_or_deletion() {
    use crate::publication_faults::{Cut, Side, with_error_at};

    let dir = tempfile::tempdir().expect("state root");
    let admission = Arc::new(RecordingAdmission::default());
    let store = ObjectStore::open(dir.path())
        .expect("pool bootstrap")
        .with_pool_write_admission(Some(admission.clone()));
    let value = {
        let _gate = store.lock_gate().expect("pool gate");
        store
            .put_object_locked(ObjectKind::Record, &[], b"unreachable record")
            .expect("physical object")
    };
    let epoch = store.read_epoch().expect("epoch");
    let (result, fired) = with_error_at(Cut::PoolGcDirectorySync, Side::Before, 1, || {
        store.gc_sweep_batch(
            epoch,
            &[value.sha256],
            &std::collections::BTreeSet::default(),
        )
    });
    assert!(fired && result.is_err());
    let path = canonical_path(dir.path(), value.sha256);
    fs::write(&path, b"foreign replacement").expect("out of band file");
    assert!(matches!(
        store.object_inventory_cursor(),
        Err(CoreError::Typed {
            code: quanta_index_core::GENERATION_SIDECAR_CORRUPT_CODE,
            ..
        })
    ));
    assert_eq!(
        fs::read(path).expect("foreign file retained"),
        b"foreign replacement"
    );
    assert_eq!(
        admission
            .snapshot()
            .expect("retained usage")
            .canonical_bytes,
        value.bytes
    );
    assert!(
        admission
            .0
            .lock()
            .expect("recording lock")
            .canonical_releases
            .is_empty()
    );
}

#[test]
fn unmarked_missing_canonical_is_not_automatically_refunded_or_recreated() {
    let dir = tempfile::tempdir().expect("state root");
    let admission = Arc::new(RecordingAdmission::default());
    let store = ObjectStore::open(dir.path())
        .expect("pool bootstrap")
        .with_pool_write_admission(Some(admission.clone()));
    let value = {
        let _gate = store.lock_gate().expect("pool gate");
        store
            .put_object_locked(ObjectKind::Record, &[], b"live record")
            .expect("physical object")
    };
    let path = canonical_path(dir.path(), value.sha256);
    fs::remove_file(&path).expect("out of band removal without GC marker");
    let result = {
        let _gate = store.lock_gate().expect("pool gate");
        store.put_object_locked(ObjectKind::Record, &[], b"live record")
    };
    assert!(result.is_err());
    assert!(!path.exists());
    assert_eq!(
        admission
            .snapshot()
            .expect("retained charge")
            .canonical_bytes,
        value.bytes
    );
    assert!(
        admission
            .0
            .lock()
            .expect("recording lock")
            .canonical_releases
            .is_empty()
    );
}
