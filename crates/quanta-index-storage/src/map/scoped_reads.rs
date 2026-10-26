//! One funded read window over immutable original map nodes.

use std::sync::Mutex;
#[cfg(test)]
use std::sync::atomic::{AtomicU64, Ordering};

use quanta_index_core::index::{
    IndexError, MapBatchRequest, MapBatchWrite, MapEdit, MapPage, MapRoot, MapValue, MapWrite,
    MemoryLease, MemoryLifetime, ObjectRef, StagedMapRoots, StoredObject, WorkBudget,
    ports::{
        ImmutableObjectPort, MapDiffRange, MapDiffVisitor, MapEditCursor, ObjectWriteSink,
        PersistentMapPort,
    },
};

use super::{Node, charge_key_allocation, decode_node, key_nibbles};
use crate::FileObjectStore;

const SLOTS: usize = 512;
const SLOT_MASK: usize = 511;
const MAX_DATA_BYTES: u64 = 1024 * 1024;

struct Entry {
    root: MapRoot,
    reference: ObjectRef,
    bytes: u64,
    object: StoredObject,
}

struct ReadCache {
    entries: [Option<Entry>; SLOTS],
    bytes: u64,
    hand: usize,
    _table_memory: Option<MemoryLease>,
}

impl ReadCache {
    fn new(budget: &WorkBudget) -> Result<Box<Self>, IndexError> {
        let bytes = u64::try_from(std::mem::size_of::<Self>())
            .map_err(|_error| IndexError::ResourceLimited)?;
        let table_memory = budget.reserve_memory(bytes, MemoryLifetime::Temporary)?;
        Ok(Box::new(Self {
            entries: std::array::from_fn(|_index| None),
            bytes: 0,
            hand: 0,
            _table_memory: table_memory,
        }))
    }

    fn slot(root: MapRoot, reference: ObjectRef) -> usize {
        let ref_bytes = &reference.0.0;
        let first = usize::from(ref_bytes.first().copied().unwrap_or_default());
        let second = usize::from(ref_bytes.get(1).copied().unwrap_or_default());
        let root_first = usize::from(
            root.0
                .and_then(|value| value.0.0.first().copied())
                .unwrap_or_default(),
        );
        (first.wrapping_shl(1) ^ second ^ root_first) & SLOT_MASK
    }

    fn get(&self, root: MapRoot, reference: ObjectRef) -> Option<StoredObject> {
        self.entries
            .get(Self::slot(root, reference))
            .and_then(Option::as_ref)
            .filter(|entry| entry.root == root && entry.reference == reference)
            .map(|entry| entry.object.clone())
    }

    fn remove(&mut self, index: usize) -> Result<(), IndexError> {
        if let Some(old) = self.entries.get_mut(index).and_then(Option::take) {
            self.bytes = self.bytes.checked_sub(old.bytes).ok_or_else(|| {
                IndexError::BackendViolation("scoped map read accounting underflow".into())
            })?;
            drop(old);
        }
        Ok(())
    }

    fn insert(
        &mut self,
        root: MapRoot,
        reference: ObjectRef,
        object: StoredObject,
    ) -> Result<(), IndexError> {
        let bytes = object.memory_bytes().map_or_else(
            || {
                StoredObject::allocation_bytes(
                    object.payload.capacity(),
                    object.registration.children.len(),
                )
            },
            Ok,
        )?;
        if bytes > MAX_DATA_BYTES {
            return Ok(());
        }
        let index = Self::slot(root, reference);
        self.remove(index)?;
        while self
            .bytes
            .checked_add(bytes)
            .is_none_or(|total| total > MAX_DATA_BYTES)
        {
            self.remove(self.hand)?;
            let next = self
                .hand
                .checked_add(1)
                .ok_or(IndexError::ResourceLimited)?;
            self.hand = if next == SLOTS { 0 } else { next };
        }
        self.bytes = self
            .bytes
            .checked_add(bytes)
            .ok_or(IndexError::ResourceLimited)?;
        let slot = self
            .entries
            .get_mut(index)
            .ok_or(IndexError::ResourceLimited)?;
        *slot = Some(Entry {
            root,
            reference,
            bytes,
            object,
        });
        Ok(())
    }
}

/// The cache owns original `StoredObject` Arcs, not decoded nodes or authority.
/// Field order drops cached DATA before its table and box grants.
pub(super) struct ScopedReads<'a> {
    store: &'a FileObjectStore,
    cache: Mutex<Option<Box<ReadCache>>>,
    #[cfg(test)]
    physical_gets: AtomicU64,
    _box_memory: Option<MemoryLease>,
}

impl<'a> ScopedReads<'a> {
    pub(super) fn new(store: &'a FileObjectStore, budget: &WorkBudget) -> Result<Self, IndexError> {
        let bytes = u64::try_from(std::mem::size_of::<Self>())
            .map_err(|_error| IndexError::ResourceLimited)?;
        let box_memory = budget.reserve_memory(bytes, MemoryLifetime::Temporary)?;
        Ok(Self {
            store,
            cache: Mutex::new(None),
            #[cfg(test)]
            physical_gets: AtomicU64::new(0),
            _box_memory: box_memory,
        })
    }

    #[cfg(test)]
    pub(super) fn physical_gets(&self) -> u64 {
        self.physical_gets.load(Ordering::Relaxed)
    }

    fn read_node(
        &self,
        root: MapRoot,
        reference: ObjectRef,
        budget: &mut WorkBudget,
    ) -> Result<Node, IndexError> {
        let retained = {
            let mut cache = self.cache.lock().map_err(|_error| {
                IndexError::BackendViolation("scoped map read lock poisoned".into())
            })?;
            if cache.is_none() {
                *cache = Some(ReadCache::new(budget)?);
            }
            cache.as_ref().and_then(|table| table.get(root, reference))
        };
        if let Some(object) = retained {
            let read = self.store.read_retained(&object, budget)?;
            return decode_node(&read, budget);
        }
        let read = self.store.get(reference, budget)?;
        #[cfg(test)]
        let _previous = self.physical_gets.fetch_add(1, Ordering::Relaxed);
        let node = decode_node(&read, budget)?;
        self.cache
            .lock()
            .map_err(|_error| IndexError::BackendViolation("scoped map read lock poisoned".into()))?
            .as_mut()
            .ok_or_else(|| IndexError::BackendViolation("scoped map read table absent".into()))?
            .insert(root, reference, read)?;
        Ok(node)
    }
}

impl PersistentMapPort for ScopedReads<'_> {
    fn lookup(
        &self,
        root: MapRoot,
        key: &[u8],
        budget: &mut WorkBudget,
    ) -> Result<Option<MapValue>, IndexError> {
        budget.checkpoint()?;
        charge_key_allocation(key, budget)?;
        let nibbles = key_nibbles(key, budget)?;
        self.store
            .lookup_inner_with(root, &nibbles, budget, |reference, budget| {
                self.read_node(root, reference, budget)
            })
    }

    fn lookup_batch(
        &self,
        root: MapRoot,
        keys: &[&[u8]],
        values: &mut [Option<MapValue>],
        budget: &mut WorkBudget,
    ) -> Result<(), IndexError> {
        self.store.lookup_batch(root, keys, values, budget)
    }

    fn scan(
        &self,
        root: MapRoot,
        after: Option<&[u8]>,
        limit: u32,
        budget: &mut WorkBudget,
    ) -> Result<MapPage, IndexError> {
        self.store.scan(root, after, limit, budget)
    }

    fn visit_diff(
        &self,
        before: MapRoot,
        after: MapRoot,
        range: MapDiffRange<'_>,
        visitor: &mut dyn MapDiffVisitor,
        budget: &mut WorkBudget,
    ) -> Result<bool, IndexError> {
        self.store.visit_diff(before, after, range, visitor, budget)
    }

    fn apply(
        &self,
        base: MapRoot,
        edits: &[MapEdit],
        sink: &dyn ObjectWriteSink,
        budget: &mut WorkBudget,
    ) -> Result<MapWrite, IndexError> {
        self.store.apply(base, edits, sink, budget)
    }

    fn apply_batch(
        &self,
        requests: &[MapBatchRequest],
        sink: &dyn ObjectWriteSink,
        budget: &mut WorkBudget,
    ) -> Result<MapBatchWrite, IndexError> {
        self.store.apply_batch(requests, sink, budget)
    }

    fn apply_stream(
        &self,
        bases: &[MapRoot],
        edits: &mut dyn MapEditCursor,
        sink: &dyn ObjectWriteSink,
        budget: &mut WorkBudget,
    ) -> Result<StagedMapRoots, IndexError> {
        self.store.apply_stream(bases, edits, sink, budget)
    }
}

#[cfg(test)]
mod tests {
    use quanta_index_core::index::{Digest, RegisteredObject};

    use super::*;

    fn object(reference: ObjectRef, payload: usize) -> Result<StoredObject, IndexError> {
        Ok(StoredObject::new(
            RegisteredObject::unscoped(
                reference,
                u64::try_from(payload).map_err(|_error| IndexError::ResourceLimited)?,
                Vec::new(),
            ),
            vec![0; payload],
            None,
        ))
    }

    #[test]
    fn direct_slots_compare_complete_root_and_reference_and_evict_within_cap()
    -> Result<(), IndexError> {
        let work = WorkBudget::new(
            quanta_index_core::index::WorkLimits {
                reads: 1,
                writes: 1,
                bytes: 1,
                mutations: 1,
            },
            std::time::Instant::now()
                .checked_add(std::time::Duration::from_secs(30))
                .ok_or(IndexError::ResourceLimited)?,
        );
        let mut cache = ReadCache::new(&work)?;
        let first = ObjectRef(Digest([1; 32]));
        let mut altered = [1; 32];
        *altered.last_mut().ok_or(IndexError::ResourceLimited)? = 2;
        let collision = ObjectRef(Digest(altered));
        let root = MapRoot(Some(ObjectRef(Digest([7; 32]))));
        let mut other_root_bytes = [7; 32];
        *other_root_bytes
            .last_mut()
            .ok_or(IndexError::ResourceLimited)? = 8;
        let other_root = MapRoot(Some(ObjectRef(Digest(other_root_bytes))));
        cache.insert(root, first, object(first, 32)?)?;
        if cache.get(root, first).is_none()
            || cache.get(MapRoot(None), first).is_some()
            || cache.get(other_root, first).is_some()
            || cache.get(root, collision).is_some()
        {
            return Err(IndexError::BackendViolation(
                "scoped cache key comparison differs".into(),
            ));
        }
        cache.insert(root, collision, object(collision, 32)?)?;
        if cache.get(root, first).is_some() || cache.get(root, collision).is_none() {
            return Err(IndexError::BackendViolation(
                "scoped collision did not evict".into(),
            ));
        }
        let oversized = ObjectRef(Digest([3; 32]));
        cache.insert(
            root,
            oversized,
            object(
                oversized,
                usize::try_from(MAX_DATA_BYTES).map_err(|_error| IndexError::ResourceLimited)?,
            )?,
        )?;
        if cache.get(root, oversized).is_some() {
            return Err(IndexError::BackendViolation(
                "oversized DATA entered cache".into(),
            ));
        }
        for id in 4_u8..12 {
            let reference = ObjectRef(Digest([id; 32]));
            cache.insert(root, reference, object(reference, 256 * 1024)?)?;
            if cache.bytes > MAX_DATA_BYTES || cache.get(root, reference).is_none() {
                return Err(IndexError::BackendViolation(
                    "scoped DATA cap differs".into(),
                ));
            }
        }
        Ok(())
    }
}
