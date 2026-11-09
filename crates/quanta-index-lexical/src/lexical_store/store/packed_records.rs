//! Bounded shared physical roots for small, independently authenticated payloads.
//!
//! `BlobRef` keeps each logical payload's length and digest. A pack authenticates
//! every entry in digest order; selecting one never changes its logical bytes.

use quanta_index_core::CoreError;
use sha2::{Digest as _, Sha256};

use super::{
    AttemptLease, BLOCK_BYTES, BlobRef, ObjectKind, ObjectRef, ObjectStore, corrupt, invalid,
    storage,
};

pub(crate) const MAX_PACKED_RECORDS: usize = 96;
const BODY_GROUP_BYTES: usize = 16 * 1024;

pub(super) fn packed_dependencies_valid(kind: ObjectKind, dependencies: &[ObjectRef]) -> bool {
    match (kind, dependencies) {
        (ObjectKind::Record | ObjectKind::NativeBody | ObjectKind::FileBody, []) => true,
        (ObjectKind::Record, [body]) => {
            matches!(body.kind, ObjectKind::NativeBody | ObjectKind::FileBody)
        }
        _ => false,
    }
}

impl ObjectStore {
    /// At most one 32 KiB payload and 96 logical references are staged here.
    /// Singleton/wide records retain the ordinary canonical blob encoding.
    pub(crate) fn put_record_group<T: AsRef<[u8]>>(
        &self,
        lease: &AttemptLease,
        records: &[T],
    ) -> Result<Vec<BlobRef>, CoreError> {
        self.put_payload_group(lease, ObjectKind::Record, &[], records)
    }

    /// All records in each pack name the same authenticated body root.
    pub(crate) fn put_body_record_groups<T: AsRef<[u8]>>(
        &self,
        lease: &AttemptLease,
        body: &ObjectRef,
        records: &[T],
    ) -> Result<Vec<BlobRef>, CoreError> {
        if !matches!(body.kind, ObjectKind::NativeBody | ObjectKind::FileBody) {
            return Err(invalid("packed body records require a body dependency"));
        }
        self.put_payload_groups(
            lease,
            ObjectKind::Record,
            std::slice::from_ref(body),
            records,
            BLOCK_BYTES,
        )
    }

    /// Pack adjacent small bodies without changing their order or logical bytes.
    /// Empty and wide bodies retain the independently deduplicated encoding.
    pub(crate) fn put_body_groups<T: AsRef<[u8]>>(
        &self,
        lease: &AttemptLease,
        kind: ObjectKind,
        bodies: &[T],
    ) -> Result<Vec<BlobRef>, CoreError> {
        if !matches!(kind, ObjectKind::NativeBody | ObjectKind::FileBody) {
            return Err(invalid("body packing requires a body object kind"));
        }
        self.put_payload_groups(lease, kind, &[], bodies, BODY_GROUP_BYTES)
    }

    fn put_payload_groups<T: AsRef<[u8]>>(
        &self,
        lease: &AttemptLease,
        kind: ObjectKind,
        dependencies: &[ObjectRef],
        payloads: &[T],
        max_group_bytes: usize,
    ) -> Result<Vec<BlobRef>, CoreError> {
        if !packed_dependencies_valid(kind, dependencies) {
            return Err(invalid("packed payload has invalid kind or dependencies"));
        }
        let mut output = Vec::new();
        output
            .try_reserve_exact(payloads.len())
            .map_err(|error| storage("reserve packed payload references", error))?;
        let mut remaining = payloads;
        while let Some((first, rest)) = remaining.split_first() {
            let first = first.as_ref();
            if first.is_empty() || first.len() > max_group_bytes - 5 {
                output.push(self.put_blob(lease, kind, dependencies, first)?);
                remaining = rest;
                continue;
            }
            let mut width = 3_usize;
            let count = remaining
                .iter()
                .take(MAX_PACKED_RECORDS)
                .take_while(|body| {
                    let len = body.as_ref().len();
                    width = width.saturating_add(2).saturating_add(len);
                    len != 0 && width <= max_group_bytes
                })
                .count();
            let (group, rest) = remaining.split_at(count);
            output.extend(self.put_payload_group(lease, kind, dependencies, group)?);
            remaining = rest;
        }
        Ok(output)
    }

    fn put_payload_group<T: AsRef<[u8]>>(
        &self,
        lease: &AttemptLease,
        kind: ObjectKind,
        dependencies: &[ObjectRef],
        records: &[T],
    ) -> Result<Vec<BlobRef>, CoreError> {
        if !packed_dependencies_valid(kind, dependencies) {
            return Err(invalid("packed payload has invalid kind or dependencies"));
        }
        if records.is_empty() || records.len() > MAX_PACKED_RECORDS {
            return Err(invalid("packed record count exceeds bound"));
        }
        lease.ensure_store(self)?;
        let mut entries: Vec<([u8; 32], &[u8])> = Vec::new();
        entries
            .try_reserve_exact(records.len())
            .map_err(|e| storage("reserve packed records", e))?;
        for record in records {
            let record = record.as_ref();
            if record.len() > BLOCK_BYTES {
                return Err(invalid("packed logical record exceeds block bound"));
            }
            entries.push((Sha256::digest(record).into(), record));
        }
        entries.sort_unstable_by_key(|(digest, _)| *digest);
        for pair in entries.windows(2) {
            let [left, right] = pair else {
                return Err(invalid("packed record window width"));
            };
            if left.0 == right.0 && left.1 != right.1 {
                return Err(corrupt("packed logical digest collision"));
            }
        }
        entries.dedup_by_key(|(digest, _)| *digest);
        if entries.len() == 1 {
            let (_, bytes) = entries
                .first()
                .ok_or_else(|| invalid("packed record absent"))?;
            let reference = self.put_blob(lease, kind, dependencies, bytes)?;
            return Ok(vec![reference; records.len()]);
        }
        let width = entries.iter().try_fold(3_usize, |n, (_, bytes)| {
            n.checked_add(2)
                .and_then(|n| n.checked_add(bytes.len()))
                .ok_or_else(|| invalid("packed record width overflow"))
        })?;
        if width > BLOCK_BYTES {
            return Err(invalid("packed record payload exceeds block bound"));
        }
        let mut payload = Vec::new();
        payload
            .try_reserve_exact(width)
            .map_err(|e| storage("reserve packed payload", e))?;
        payload.push(2);
        payload.extend_from_slice(
            &u16::try_from(entries.len())
                .map_err(|e| storage("packed count", e))?
                .to_be_bytes(),
        );
        for (_, bytes) in entries {
            payload.extend_from_slice(
                &u16::try_from(bytes.len())
                    .map_err(|e| storage("packed width", e))?
                    .to_be_bytes(),
            );
            payload.extend_from_slice(bytes);
        }
        let root = self.put_adopted_object(lease, kind, dependencies, &payload)?;
        let mut references = Vec::new();
        references
            .try_reserve_exact(records.len())
            .map_err(|e| storage("reserve packed references", e))?;
        for bytes in records {
            let bytes = bytes.as_ref();
            references.push(BlobRef {
                root: root.clone(),
                bytes: u64::try_from(bytes.len())
                    .map_err(|e| storage("logical record width", e))?,
                sha256: Sha256::digest(bytes).into(),
            });
        }
        Ok(references)
    }
}

/// An authenticated, bounded index into one immutable Record payload.
/// Offsets are relative to the payload after its tag byte.
pub(super) struct RecordGroupIndex {
    entries: Vec<([u8; 32], std::ops::Range<usize>)>,
    payload_bytes: usize,
}

impl RecordGroupIndex {
    fn decode(bytes: &[u8]) -> Result<Self, CoreError> {
        if bytes.len().checked_add(1).is_none_or(|n| n > BLOCK_BYTES) {
            return Err(corrupt("packed record exceeds physical bound"));
        }
        let count_bytes: [u8; 2] = bytes
            .get(..2)
            .ok_or_else(|| corrupt("packed count missing"))?
            .try_into()
            .map_err(|e| storage("packed count width", e))?;
        let count = usize::from(u16::from_be_bytes(count_bytes));
        if !(2..=MAX_PACKED_RECORDS).contains(&count) {
            return Err(corrupt("packed record count differs"));
        }
        let mut entries = Vec::new();
        entries
            .try_reserve_exact(count)
            .map_err(|e| storage("reserve packed index", e))?;
        let mut offset = 2_usize;
        let mut previous = None;
        for _ in 0..count {
            let end = offset
                .checked_add(2)
                .ok_or_else(|| corrupt("packed width overflow"))?;
            let length: [u8; 2] = bytes
                .get(offset..end)
                .ok_or_else(|| corrupt("packed width missing"))?
                .try_into()
                .map_err(|e| storage("packed width", e))?;
            let length = usize::from(u16::from_be_bytes(length));
            offset = end;
            let end = offset
                .checked_add(length)
                .ok_or_else(|| corrupt("packed extent overflow"))?;
            let record = bytes
                .get(offset..end)
                .ok_or_else(|| corrupt("packed entry truncated"))?;
            let digest: [u8; 32] = Sha256::digest(record).into();
            if previous.is_some_and(|prior| prior >= digest) {
                return Err(corrupt("packed records are not uniquely digest ordered"));
            }
            previous = Some(digest);
            entries.push((digest, offset..end));
            offset = end;
        }
        if offset != bytes.len() {
            return Err(corrupt("packed record has trailing bytes"));
        }
        Ok(Self {
            entries,
            payload_bytes: bytes.len(),
        })
    }

    pub(super) fn charge(&self) -> Option<usize> {
        self.entries
            .capacity()
            .checked_mul(std::mem::size_of::<([u8; 32], std::ops::Range<usize>)>())?
            .checked_add(std::mem::size_of::<Self>())?
            .checked_add(std::mem::size_of::<[usize; 2]>())
    }

    pub(super) fn select<'a>(
        &self,
        bytes: &'a [u8],
        expected_length: u64,
        digest: &[u8; 32],
    ) -> Result<&'a [u8], CoreError> {
        if bytes.len() != self.payload_bytes {
            return Err(corrupt("packed cached payload width differs"));
        }
        let index = self
            .entries
            .binary_search_by_key(digest, |(digest, _)| *digest)
            .map_err(|_absent| corrupt("packed root does not contain logical digest"))?;
        let (_, range) = self
            .entries
            .get(index)
            .ok_or_else(|| corrupt("packed index disappeared"))?;
        let selected = bytes
            .get(range.clone())
            .ok_or_else(|| corrupt("packed cached extent differs"))?;
        if u64::try_from(selected.len()).map_err(|e| storage("packed logical width", e))?
            != expected_length
        {
            return Err(corrupt("packed logical length differs"));
        }
        Ok(selected)
    }
}

impl ObjectStore {
    pub(super) fn record_group_index(
        &self,
        reference: &super::ObjectRef,
        bytes: &[u8],
    ) -> Result<std::sync::Arc<RecordGroupIndex>, CoreError> {
        self.require_prepared_batch_open()?;
        if let Some(meter) = &self.read_meter {
            let mut cache = meter
                .objects_cache
                .lock()
                .map_err(|e| storage("lock packed record cache", e))?;
            if let Some(index) = cache.get_record_group(reference)? {
                return Ok(index);
            }
        }
        // Only called after read_object authenticated this exact root. The
        // complete pack is checked once; each logical read still checks its
        // own committed length and final digest. Independent meters start empty.
        let index = std::sync::Arc::new(RecordGroupIndex::decode(bytes)?);
        if let Some(meter) = &self.read_meter {
            meter
                .objects_cache
                .lock()
                .map_err(|e| storage("lock packed record cache", e))?
                .attach_record_group(reference, &index)?;
        }
        Ok(index)
    }
}

#[cfg(test)]
fn select_record<'a>(
    bytes: &'a [u8],
    length: u64,
    digest: &[u8; 32],
) -> Result<&'a [u8], CoreError> {
    RecordGroupIndex::decode(bytes)?.select(bytes, length, digest)
}

#[cfg(test)]
mod tests {
    use super::super::AttemptBinding;
    use super::*;

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
    fn cold_pack_and_cursor_preserve_fixed_logical_bytes_and_exact_wire() {
        let dir = tempfile::tempdir().expect("directory");
        let store = ObjectStore::open(dir.path()).expect("store");
        let lease = store.begin_attempt(&binding(), &[]).expect("attempt");
        let inputs = vec![b"alpha".to_vec(), b"beta".to_vec(), b"alpha".to_vec()];
        let refs = store.put_record_group(&lease, &inputs).expect("pack");
        assert!(refs.iter().all(|r| r.root == refs[0].root));
        assert_eq!(refs[0], refs[2]);
        let cold = ObjectStore::open(dir.path()).expect("cold store");
        let (deps, wire) = cold
            .read_object(&refs[0].root, None)
            .expect("persisted pack");
        assert!(deps.is_empty());
        assert_eq!(wire, b"\x02\x00\x02\x00\x05alpha\x00\x04beta");
        assert_eq!(refs[0].root.bytes, 37);
        for (reference, expected) in refs.iter().zip(&inputs) {
            assert_eq!(
                cold.read_blob(reference, 5, None)
                    .expect("cold logical read"),
                *expected
            );
            let mut cursor = cold.blob_cursor(reference, 5).expect("cursor");
            let (actual, charged) = cursor.next(&cold, None).expect("block").expect("payload");
            assert_eq!(&actual, expected);
            assert_eq!(charged, 37);
            assert!(
                cursor
                    .next(&cold, None)
                    .expect("complete authentication")
                    .is_none()
            );
        }
        let mut missing = refs[0].clone();
        missing.sha256 = [0; 32];
        assert!(cold.read_blob(&missing, 5, None).is_err());
        assert!(
            cold.blob_cursor(&missing, 5)
                .expect("cursor")
                .next(&cold, None)
                .is_err()
        );
        let mut wrong_length = refs[0].clone();
        wrong_length.bytes = 4;
        assert!(cold.read_blob(&wrong_length, 5, None).is_err());
    }

    #[test]
    fn cold_body_packs_preserve_members_and_shared_native_dependency() {
        let dir = tempfile::tempdir().expect("directory");
        let store = ObjectStore::open(dir.path()).expect("store");
        let lease = store.begin_attempt(&binding(), &[]).expect("attempt");
        let wide = vec![42_u8; BLOCK_BYTES + 1];
        let inputs = [
            b"alpha".as_slice(),
            b"beta".as_slice(),
            &wide,
            b"".as_slice(),
        ];
        for kind in [ObjectKind::NativeBody, ObjectKind::FileBody] {
            let refs = store
                .put_body_groups(&lease, kind, &inputs)
                .expect("body groups");
            assert_eq!(refs.len(), inputs.len());
            assert_eq!(refs[0].root, refs[1].root);
            assert_ne!(refs[0].sha256, refs[1].sha256);
            assert_ne!(refs[0].root, refs[2].root);
            assert_eq!(
                refs[3],
                store
                    .put_blob(&lease, kind, &[], b"")
                    .expect("empty singleton")
            );
            let cold = ObjectStore::open(dir.path())
                .expect("cold store")
                .metered_clone();
            for (reference, expected) in refs.iter().zip(inputs) {
                assert_eq!(
                    cold.read_blob(reference, BLOCK_BYTES as u64 + 1, None)
                        .expect("logical body"),
                    expected
                );
            }
            let mut cursor = cold.blob_cursor(&refs[1], 4).expect("cursor");
            assert_eq!(
                cursor.next(&cold, None).expect("read").expect("block").0,
                b"beta"
            );
            assert!(cursor.next(&cold, None).expect("finished").is_none());
            let mut missing = refs[0].clone();
            missing.sha256 = [0; 32];
            assert!(cold.read_blob(&missing, 5, None).is_err());

            if kind == ObjectKind::NativeBody {
                let records = store
                    .put_body_record_groups(&lease, &refs[0].root, &[b"one", b"two"])
                    .expect("dependent Record pack");
                assert_eq!(records[0].root, records[1].root);
                assert_eq!(
                    cold.read_blob(&records[1], 3, None)
                        .expect("logical record"),
                    b"two"
                );
                let mut reachable = std::collections::BTreeSet::new();
                cold.walk_closure(&[records[1].root.clone()], None, &mut |reference| {
                    let _new = reachable.insert(reference.clone());
                    Ok(())
                })
                .expect("record-to-body closure");
                assert_eq!(
                    reachable,
                    [records[1].root.clone(), refs[1].root.clone()]
                        .into_iter()
                        .collect()
                );
            }
        }
    }

    #[test]
    fn packed_shape_refuses_reordering_duplicates_truncation_and_trailing_bytes() {
        let digest: [u8; 32] = Sha256::digest(b"alpha").into();
        let valid = b"\x00\x02\x00\x05alpha\x00\x04beta";
        assert_eq!(select_record(valid, 5, &digest).expect("valid"), b"alpha");
        for malformed in [
            b"\x00\x02\x00\x04beta\x00\x05alpha".as_slice(),
            b"\x00\x02\x00\x05alpha\x00\x05alpha".as_slice(),
            b"\x00\x02\x00\x05alpha\x00\x04bet".as_slice(),
            b"\x00\x02\x00\x05alpha\x00\x04betaX".as_slice(),
            b"\x00\x01\x00\x05alpha".as_slice(),
            b"\x00\x61".as_slice(),
        ] {
            assert!(select_record(malformed, 5, &digest).is_err());
        }
    }

    #[test]
    fn metered_pack_reads_preserve_logical_checks_eviction_and_fresh_authentication() {
        let dir = tempfile::tempdir().expect("directory");
        let store = ObjectStore::open(dir.path()).expect("store");
        let lease = store.begin_attempt(&binding(), &[]).expect("attempt");
        let inputs = (0..96)
            .map(|n| format!("{n:02}:alpha:{}", "x".repeat(70)))
            .collect::<Vec<_>>();
        let other = (0..96)
            .map(|n| format!("{n:02}:beta_:{}", "x".repeat(70)))
            .collect::<Vec<_>>();
        let first = store.put_record_group(&lease, &inputs).expect("first pack");
        let second = store.put_record_group(&lease, &other).expect("second pack");
        let query = store.metered_clone();
        *query
            .read_meter
            .as_ref()
            .expect("meter")
            .objects_cache
            .lock()
            .expect("cache") = super::super::RequestObjectCache::new(20 * 1024);
        for (reference, expected) in first.iter().zip(&inputs) {
            assert_eq!(
                query.read_blob(reference, 128, None).expect("logical read"),
                expected.as_bytes()
            );
        }
        assert_eq!(query.read_meter_snapshot().1, 1);
        assert!(
            query
                .object_dependencies(&first[0].root, None)
                .expect("authenticated cached dependencies")
                .is_empty()
        );
        assert_eq!(query.read_meter_snapshot().1, 1);
        let mut wrong_length = first[0].clone();
        wrong_length.bytes += 1;
        assert!(query.read_blob(&wrong_length, 128, None).is_err());
        let mut absent = first[0].clone();
        absent.sha256 = [0; 32];
        assert!(query.read_blob(&absent, 128, None).is_err());
        assert_eq!(
            query.read_blob(&second[0], 128, None).expect("other pack"),
            other[0].as_bytes()
        );
        assert_eq!(
            query
                .read_blob(&first[0], 128, None)
                .expect("evicted pack reloads"),
            inputs[0].as_bytes()
        );
        assert_eq!(
            query.read_meter_snapshot().1,
            3,
            "decoded pack metadata participates in the same cache bound"
        );
        let cache = query
            .read_meter
            .as_ref()
            .expect("meter")
            .objects_cache
            .lock()
            .expect("cache");
        assert!(cache.retained_bytes <= 20 * 1024);
        drop(cache);
        let digest = super::super::hex_digest(&first[0].root.sha256);
        let path = dir
            .path()
            .join(super::super::POOL_DIR)
            .join(super::super::OBJECTS_DIR)
            .join(digest.get(..2).expect("shard"))
            .join(digest);
        let mut bytes = std::fs::read(&path).expect("pack bytes");
        *bytes.last_mut().expect("nonempty pack") ^= 1;
        std::fs::write(path, bytes).expect("same-width corruption");
        assert!(
            store
                .metered_clone()
                .read_blob(&first[0], 128, None)
                .is_err(),
            "a fresh independent request must authenticate the complete physical root"
        );
    }

    #[test]
    fn packing_bounds_refuse_before_birth_and_wide_singleton_stays_readable() {
        let dir = tempfile::tempdir().expect("directory");
        let store = ObjectStore::open(dir.path()).expect("store");
        let lease = store.begin_attempt(&binding(), &[]).expect("attempt");
        let before = lease.write_cost_snapshot().expect("cost");
        assert!(store.put_record_group(&lease, &vec![vec![1]; 97]).is_err());
        assert!(
            store
                .put_record_group(&lease, &[vec![1; 16384], vec![2; 16384]])
                .is_err()
        );
        assert_eq!(lease.write_cost_snapshot().expect("refused cost"), before);
        let bytes = vec![7; 32768];
        let references = store
            .put_record_group(&lease, std::slice::from_ref(&bytes))
            .expect("singleton");
        assert_eq!(
            store.read_blob(&references[0], 32768, None).expect("read"),
            bytes
        );
        assert_eq!(references[0].root.bytes, 32790);
    }
}
