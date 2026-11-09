//! Pack a bounded group of logical Record edits before physical publication.

use std::collections::{BTreeMap, VecDeque};

use super::{corrupt, invalid};
use crate::lexical_store::{
    AttemptLease, MAX_PACKED_RECORDS, MAX_RECORD_GROUP_BYTES, MapEdit, ObjectStore,
};
use quanta_index_core::CoreError;

pub(super) const FRESH_LEAF_LOOKAHEAD_GROUPS: usize = 8;

pub(super) struct RecordEdit {
    pub(super) key: Vec<u8>,
    pub(super) bytes: Vec<u8>,
}

pub(super) struct PackedEdits<'a, I> {
    store: &'a ObjectStore,
    lease: &'a AttemptLease,
    source: I,
    lookahead_records: usize,
    lookahead_bytes: usize,
    pending: Option<RecordEdit>,
    ready: VecDeque<MapEdit>,
    failed: bool,
    pub(super) record_encoded_bytes: u64,
}

impl<'a, I: Iterator<Item = Result<RecordEdit, CoreError>>> PackedEdits<'a, I> {
    pub(super) fn new(store: &'a ObjectStore, lease: &'a AttemptLease, source: I) -> Self {
        Self {
            store,
            lease,
            source,
            lookahead_records: MAX_PACKED_RECORDS,
            lookahead_bytes: MAX_RECORD_GROUP_BYTES,
            pending: None,
            ready: VecDeque::new(),
            failed: false,
            record_encoded_bytes: 0,
        }
    }

    /// Collect more sorted fresh-leaf edits before partitioning by the same
    /// surface and logical-ID bucket. Physical groups keep the store's limits.
    #[cfg(test)]
    pub(super) fn fresh_leaf_lookahead(mut self) -> Self {
        self.lookahead_records = MAX_PACKED_RECORDS * FRESH_LEAF_LOOKAHEAD_GROUPS;
        self.lookahead_bytes = MAX_RECORD_GROUP_BYTES * FRESH_LEAF_LOOKAHEAD_GROUPS;
        self
    }

    fn publish_group(
        &mut self,
        indices: &[usize],
        records: &[Vec<u8>],
        references: &mut [Option<crate::lexical_store::BlobRef>],
    ) -> Result<(), CoreError> {
        let mut members = Vec::new();
        members
            .try_reserve_exact(indices.len())
            .map_err(|e| CoreError::Storage(format!("reserve Record members: {e}")))?;
        for &index in indices {
            members.push(
                records
                    .get(index)
                    .ok_or_else(|| corrupt("packed Record member absent"))?
                    .as_slice(),
            );
        }
        let packed = self.store.put_record_group(self.lease, &members)?;
        if packed.len() != indices.len() {
            return Err(corrupt("packed reference count differs"));
        }
        for (&index, reference) in indices.iter().zip(packed) {
            *references
                .get_mut(index)
                .ok_or_else(|| corrupt("packed reference slot absent"))? = Some(reference);
        }
        Ok(())
    }

    fn fill(&mut self) -> Result<(), CoreError> {
        let mut keys = Vec::new();
        let mut records = Vec::new();
        let mut width = 3_usize;
        for _ in 0..self.lookahead_records {
            let row = match self.pending.take() {
                Some(row) => Some(row),
                None => self.source.next().transpose()?,
            };
            let Some(row) = row else { break };
            if row.bytes.len() > MAX_RECORD_GROUP_BYTES {
                return Err(invalid("logical Record exceeds packing bound"));
            }
            let next_width = width
                .checked_add(2)
                .and_then(|n| n.checked_add(row.bytes.len()))
                .ok_or_else(|| invalid("Record group width overflow"))?;
            if !records.is_empty() && next_width > self.lookahead_bytes {
                self.pending = Some(row);
                break;
            }
            keys.try_reserve(1)
                .map_err(|e| CoreError::Storage(format!("reserve packed keys: {e}")))?;
            records
                .try_reserve(1)
                .map_err(|e| CoreError::Storage(format!("reserve packed bodies: {e}")))?;
            keys.push(row.key);
            records.push(row.bytes);
            width = next_width;
        }
        if records.is_empty() {
            return Ok(());
        }
        self.ready
            .try_reserve(keys.len())
            .map_err(|e| CoreError::Storage(format!("reserve packed edits: {e}")))?;
        // Restricted posting queries must never open a different logical-ID
        // bucket or surface. Group only within that existing physical scope.
        // Map edit order remains the original gram/bucket key order.
        let mut domains: BTreeMap<Vec<u8>, Vec<usize>> = BTreeMap::new();
        for (index, key) in keys.iter().enumerate() {
            let mut domain = key
                .get(..1)
                .ok_or_else(|| corrupt("Record key has no surface"))?
                .to_vec();
            match key.len() {
                4 => {}
                12 => domain.extend_from_slice(
                    key.get(4..12)
                        .ok_or_else(|| corrupt("Record key has no bucket"))?,
                ),
                _ => return Err(corrupt("packed run Record key width differs")),
            }
            let indices = domains.entry(domain).or_default();
            indices
                .try_reserve(1)
                .map_err(|e| CoreError::Storage(format!("reserve packing domain: {e}")))?;
            indices.push(index);
        }
        let mut references = Vec::new();
        references
            .try_reserve_exact(keys.len())
            .map_err(|e| CoreError::Storage(format!("reserve Record references: {e}")))?;
        references.resize_with(keys.len(), || None);
        for indices in domains.values() {
            let mut start = 0;
            while start < indices.len() {
                let mut end = start;
                let mut width = 3_usize;
                while let Some(&index) = indices.get(end) {
                    if end - start == MAX_PACKED_RECORDS {
                        break;
                    }
                    let record = records
                        .get(index)
                        .ok_or_else(|| corrupt("packed Record member absent"))?;
                    let next_width = width
                        .checked_add(2)
                        .and_then(|n| n.checked_add(record.len()))
                        .ok_or_else(|| invalid("Record group width overflow"))?;
                    if end > start && next_width > MAX_RECORD_GROUP_BYTES {
                        break;
                    }
                    width = next_width;
                    end += 1;
                }
                self.publish_group(&indices[start..end], &records, &mut references)?;
                start = end;
            }
        }
        for (key, reference) in keys.into_iter().zip(references) {
            self.ready.push_back(MapEdit {
                key,
                value: Some(reference.ok_or_else(|| corrupt("packed logical reference absent"))?),
            });
        }
        Ok(())
    }
}

impl<I: Iterator<Item = Result<RecordEdit, CoreError>>> Iterator for PackedEdits<'_, I> {
    type Item = Result<MapEdit, CoreError>;
    fn next(&mut self) -> Option<Self::Item> {
        if self.failed {
            return None;
        }
        if self.ready.is_empty()
            && let Err(error) = self.fill()
        {
            self.failed = true;
            return Some(Err(error));
        }
        self.ready.pop_front().map(|edit| {
            let reference = edit
                .value
                .as_ref()
                .ok_or_else(|| corrupt("packed value absent"))?;
            self.record_encoded_bytes = self
                .record_encoded_bytes
                .checked_add(reference.root.bytes)
                .ok_or_else(|| invalid("packed Record root ledger overflow"))?;
            Ok(edit)
        })
    }
}
