//! One attempt's original resident owner maps and same-backend read window.

use std::sync::{Arc, Mutex};

use quanta_index_core::index::{
    AttemptRef, IndexError, MapBatchRequest, MapBatchWrite, MapEdit, MapPage, MapRoot, MapValue,
    MapWrite, MemoryLease, MemoryLifetime, ObjectBatchInput, ObjectInstallScope, ObjectRef,
    RegisteredObject, RegisteredObjectBatch, StagedMapRoots, StagedObjectReferences, StoredObject,
    WorkBudget,
    ports::{
        ImmutableObjectPort, MAX_MAP_LOOKUP_BATCH, MAX_OWNER_MAP_BATCH_OBJECTS,
        MAX_OWNER_MAP_BATCH_ROOTS, MapDiffRange, MapDiffVisitor, MapEditCursor, ObjectMapBatchPort,
        ObjectWriteSink, OwnerMapCompletion, PersistentMapPort, PreparedObjectMapBatch,
        PreparedOwnerMapScope,
    },
};

use super::{
    FileObjectStore, MapBuffer, ResidentStage, charge_key_allocation, key_nibbles, original_lookup,
};
use crate::object::MAX_BATCH_BYTES;

struct Pending {
    stage: ResidentStage,
}

enum State {
    Pending(Pending),
    Reading,
    Installing,
    Installed,
    Transferred,
    Failed,
}

pub(super) struct OwnerScope {
    store: Arc<FileObjectStore>,
    attempt: AttemptRef,
    references: [ObjectRef; 2],
    roots: [MapRoot; 2],
    initial_wire_bytes: u64,
    state: Mutex<State>,
    _memory: Option<MemoryLease>,
}

struct OwnerToken {
    store: Arc<FileObjectStore>,
    attempt: AttemptRef,
    references: [ObjectRef; 2],
    roots: [MapRoot; 2],
    initial_wire_bytes: u64,
    delta: Option<MapRoot>,
    stage: ResidentStage,
    _memory: Option<MemoryLease>,
}

fn violation(message: &'static str) -> IndexError {
    IndexError::BackendViolation(message.into())
}

fn check_scope(
    attempt: AttemptRef,
    sink: &dyn ObjectWriteSink,
    budget: &WorkBudget,
) -> Result<(), IndexError> {
    budget.checkpoint()?;
    if sink.managed_scope() != Some(ObjectInstallScope::Attempt(attempt)) {
        return Err(violation("owner scope sink attempt differs"));
    }
    Ok(())
}

impl OwnerScope {
    pub(super) fn prepare(
        store: Arc<FileObjectStore>,
        objects: &[ObjectBatchInput],
        bases: &[MapRoot],
        edits: &[&MapEdit],
        attempt: AttemptRef,
        budget: &mut WorkBudget,
    ) -> Result<Arc<dyn PreparedOwnerMapScope>, IndexError> {
        budget.checkpoint()?;
        if objects.len() != 2 || bases.len() != 2 || edits.len() != 2 {
            return Err(IndexError::ResourceLimited);
        }
        let stage = ResidentStage::owner()?;
        let mut prepared = MapBuffer::new(2);
        let mut wire_bytes = 0u64;
        for object in objects {
            let encoded = store.prepare_object(&object.payload, &object.children, budget)?;
            wire_bytes = wire_bytes
                .checked_add(encoded.registration.bytes)
                .ok_or(IndexError::ResourceLimited)?;
            if wire_bytes > MAX_BATCH_BYTES {
                return Err(IndexError::ResourceLimited);
            }
            prepared.push(encoded, budget)?;
        }
        let roots = store.prepare_resident_roots(
            bases,
            edits,
            StagedMapRoots::prepare(2, budget)?,
            &stage,
            budget,
        )?;
        let roots: [MapRoot; 2] = roots
            .as_slice()
            .try_into()
            .map_err(|_error| violation("owner scope lost metadata roots"))?;
        let references = [
            prepared
                .first()
                .ok_or_else(|| violation("owner scope lost first object"))?
                .registration
                .reference,
            prepared
                .get(1)
                .ok_or_else(|| violation("owner scope lost second object"))?
                .registration
                .reference,
        ];
        while let Some(object) = prepared.pop() {
            let _reference = stage.insert_object(object, budget)?;
        }
        let bytes = std::mem::size_of::<Self>()
            .checked_add(std::mem::size_of::<[usize; 2]>())
            .ok_or(IndexError::ResourceLimited)?;
        let bytes = u64::try_from(bytes).map_err(|_error| IndexError::ResourceLimited)?;
        let memory = budget.reserve_memory(bytes, MemoryLifetime::Retained)?;
        Ok(Arc::new(Self {
            store,
            attempt,
            references,
            roots,
            initial_wire_bytes: wire_bytes,
            state: Mutex::new(State::Pending(Pending { stage })),
            _memory: memory,
        }))
    }

    fn take_pending(&self, budget: &WorkBudget) -> Result<Option<Pending>, IndexError> {
        budget.checkpoint()?;
        let mut state = self
            .state
            .lock()
            .map_err(|_error| violation("owner scope lock poisoned"))?;
        let result = match &*state {
            State::Pending(_) => {
                let State::Pending(pending) = std::mem::replace(&mut *state, State::Installing)
                else {
                    return Err(violation("owner scope state changed"));
                };
                Ok(Some(pending))
            }
            State::Installed => Ok(None),
            State::Reading | State::Transferred | State::Installing | State::Failed => {
                Err(violation("owner scope is not available for a write"))
            }
        };
        drop(state);
        result
    }

    fn finish(&self, success: bool) -> Result<(), IndexError> {
        let mut state = self
            .state
            .lock()
            .map_err(|_error| violation("owner scope lock poisoned"))?;
        if !matches!(*state, State::Installing) {
            return Err(violation("owner scope install state changed"));
        }
        *state = if success {
            State::Installed
        } else {
            State::Failed
        };
        Ok(())
    }

    fn token(&self, pending: Pending, budget: &WorkBudget) -> Result<OwnerToken, IndexError> {
        let memory = budget.reserve_memory(
            u64::try_from(std::mem::size_of::<OwnerToken>())
                .map_err(|_error| IndexError::ResourceLimited)?,
            MemoryLifetime::Retained,
        )?;
        Ok(OwnerToken {
            store: Arc::clone(&self.store),
            attempt: self.attempt,
            references: self.references,
            roots: self.roots,
            initial_wire_bytes: self.initial_wire_bytes,
            delta: None,
            stage: pending.stage,
            _memory: memory,
        })
    }

    fn flush(&self, sink: &dyn ObjectWriteSink, budget: &mut WorkBudget) -> Result<(), IndexError> {
        check_scope(self.attempt, sink, budget)?;
        let Some(pending) = self.take_pending(budget)? else {
            return Ok(());
        };
        let result = self.token(pending, budget).and_then(|token| {
            let (refs, roots) = Box::new(token).install(&[], None, self.attempt, sink, budget)?;
            if *refs != self.references || roots.as_slice() != self.roots {
                return Err(violation("owner metadata flush changed identity"));
            }
            Ok(())
        });
        self.finish(result.is_ok())?;
        result
    }

    fn read_prepared(
        &self,
        reference: ObjectRef,
        budget: &mut WorkBudget,
    ) -> Result<Option<StoredObject>, IndexError> {
        budget.checkpoint()?;
        let mut state = self
            .state
            .lock()
            .map_err(|_error| violation("owner scope lock poisoned"))?;
        let pending = match &*state {
            State::Pending(_) => {
                let State::Pending(pending) = std::mem::replace(&mut *state, State::Reading) else {
                    return Err(violation("owner scope read state changed"));
                };
                pending
            }
            State::Installed => return Ok(None),
            State::Reading | State::Installing | State::Transferred | State::Failed => {
                return Err(violation(
                    "owner scope read after transfer or failed install",
                ));
            }
        };
        drop(state);
        // Decode/admission callbacks run outside the mutex. Reentry observes
        // Reading and fails closed until the original stage is restored.
        let result = pending.stage.read_stored(reference, budget);
        let mut state = self
            .state
            .lock()
            .map_err(|_error| violation("owner scope lock poisoned"))?;
        if !matches!(*state, State::Reading) {
            return Err(violation("owner scope read state changed"));
        }
        *state = State::Pending(pending);
        result
    }

    fn read_node(
        &self,
        reference: ObjectRef,
        budget: &mut WorkBudget,
    ) -> Result<super::Node, IndexError> {
        let staged = self.read_prepared(reference, budget)?;
        if let Some(object) = staged {
            super::decode_node(&object, budget)
        } else {
            self.store.read_node(reference, budget)
        }
    }
}

impl PreparedOwnerMapScope for OwnerScope {
    fn references(&self) -> &[ObjectRef] {
        &self.references
    }
    fn roots(&self) -> &[MapRoot] {
        &self.roots
    }
    fn objects(&self) -> &dyn ImmutableObjectPort {
        self
    }
    fn maps(&self) -> &dyn PersistentMapPort {
        self
    }
    fn writer(self: Arc<Self>) -> Arc<dyn ObjectMapBatchPort> {
        self
    }

    fn take_prepared(
        &self,
        attempt: AttemptRef,
        budget: &mut WorkBudget,
    ) -> Result<OwnerMapCompletion, IndexError> {
        budget.checkpoint()?;
        if attempt != self.attempt {
            return Err(violation("owner scope attempt differs"));
        }
        let mut state = self
            .state
            .lock()
            .map_err(|_error| violation("owner scope lock poisoned"))?;
        match &*state {
            State::Installed => Ok(OwnerMapCompletion::Installed),
            State::Transferred => Ok(OwnerMapCompletion::Transferred),
            State::Pending(_) => {
                let State::Pending(pending) = std::mem::replace(&mut *state, State::Transferred)
                else {
                    return Err(violation("owner scope state changed"));
                };
                drop(state);
                self.token(pending, budget)
                    .map(|token| OwnerMapCompletion::Prepared(Box::new(token)))
            }
            State::Reading | State::Installing | State::Failed => {
                Err(violation("owner scope cannot transfer stage"))
            }
        }
    }
}

impl PreparedObjectMapBatch for OwnerToken {
    fn root(&self) -> MapRoot {
        self.delta.unwrap_or(MapRoot(None))
    }
    fn initial_objects(&self) -> &[ObjectRef] {
        &self.references
    }
    fn initial_roots(&self) -> &[MapRoot] {
        &self.roots
    }
    fn has_prepared_root(&self) -> bool {
        self.delta.is_some()
    }

    fn install(
        self: Box<Self>,
        objects: &[ObjectBatchInput],
        additional: Option<(MapRoot, &MapEdit)>,
        attempt: AttemptRef,
        sink: &dyn ObjectWriteSink,
        budget: &mut WorkBudget,
    ) -> Result<(StagedObjectReferences, StagedMapRoots), IndexError> {
        if attempt != self.attempt {
            return Err(violation("owner token attempt differs"));
        }
        check_scope(attempt, sink, budget)?;
        if objects.len() > 2 {
            return Err(IndexError::ResourceLimited);
        }
        let mut prepared = MapBuffer::new(2);
        let mut wire_bytes = 0u64;
        for input in objects {
            let object = self
                .store
                .prepare_object(&input.payload, &input.children, budget)?;
            wire_bytes = wire_bytes
                .checked_add(object.registration.bytes)
                .ok_or(IndexError::ResourceLimited)?;
            prepared.push(object, budget)?;
        }
        if self
            .initial_wire_bytes
            .checked_add(wire_bytes)
            .ok_or(IndexError::ResourceLimited)?
            > MAX_BATCH_BYTES
        {
            return Err(IndexError::ResourceLimited);
        }
        let root_count = 2usize
            .checked_add(usize::from(self.delta.is_some()))
            .and_then(|count| count.checked_add(usize::from(additional.is_some())))
            .ok_or(IndexError::ResourceLimited)?;
        let mut roots = StagedMapRoots::prepare(root_count, budget)?;
        for root in self.roots {
            roots.push(root)?;
        }
        if let Some(delta) = self.delta {
            roots.push(delta)?;
        }
        if let Some((base, edit)) = additional {
            // This final root still checks its original base before physical I/O.
            if self.store.lookup(base, &edit.key, budget)? != edit.expected {
                return Err(IndexError::Conflict);
            }
            let next = self.store.prepare_resident_roots(
                &[base],
                &[edit],
                StagedMapRoots::prepare(1, budget)?,
                &self.stage,
                budget,
            )?;
            roots.push(
                *next
                    .first()
                    .ok_or_else(|| violation("owner final root absent"))?,
            )?;
        }
        let roots = roots.finish()?;
        let total = 2usize
            .checked_add(objects.len())
            .ok_or(IndexError::ResourceLimited)?;
        if total > MAX_OWNER_MAP_BATCH_OBJECTS || roots.len() > MAX_OWNER_MAP_BATCH_ROOTS {
            return Err(IndexError::ResourceLimited);
        }
        let memory = budget.reserve_memory(
            u64::try_from(
                total
                    .checked_mul(std::mem::size_of::<ObjectRef>())
                    .ok_or(IndexError::ResourceLimited)?,
            )
            .map_err(|_error| IndexError::ResourceLimited)?,
            MemoryLifetime::Retained,
        )?;
        let mut references = Vec::new();
        references
            .try_reserve_exact(total)
            .map_err(|_error| IndexError::ResourceLimited)?;
        references.extend(self.references);
        while let Some(object) = prepared.pop() {
            references.push(object.registration.reference);
            let _reference = self.stage.insert_object(object, budget)?;
        }
        // The LIFO staging buffer is reversed; the returned ordinal is caller order.
        references
            .get_mut(2..)
            .ok_or(IndexError::ResourceLimited)?
            .reverse();
        let objects = StagedObjectReferences::from_parts(references, memory)?;
        self.store.install_reachable_objects(
            roots.as_slice(),
            &objects,
            &self.stage,
            sink,
            budget,
        )?;
        budget.checkpoint()?;
        Ok((objects, roots))
    }
}

impl ObjectMapBatchPort for OwnerScope {
    fn install_objects_and_maps(
        &self,
        objects: &[ObjectBatchInput],
        bases: &[MapRoot],
        edits: &[&MapEdit],
        sink: &dyn ObjectWriteSink,
        budget: &mut WorkBudget,
    ) -> Result<(StagedObjectReferences, StagedMapRoots), IndexError> {
        self.flush(sink, budget)?;
        self.store
            .install_objects_and_maps(objects, bases, edits, sink, budget)
    }

    fn prepare_one_map(
        self: Arc<Self>,
        base: MapRoot,
        edit: &MapEdit,
        attempt: AttemptRef,
        budget: &mut WorkBudget,
    ) -> Result<Box<dyn PreparedObjectMapBatch>, IndexError> {
        budget.checkpoint()?;
        if attempt != self.attempt {
            return Err(violation("owner scope attempt differs"));
        }
        let mut state = self
            .state
            .lock()
            .map_err(|_error| violation("owner scope lock poisoned"))?;
        match &*state {
            State::Installed => {
                drop(state);
                Arc::clone(&self.store).prepare_one_map(base, edit, attempt, budget)
            }
            State::Pending(_) => {
                let State::Pending(pending) = std::mem::replace(&mut *state, State::Transferred)
                else {
                    return Err(violation("owner scope state changed"));
                };
                drop(state);
                let mut token = self.token(pending, budget)?;
                if self.store.lookup(base, &edit.key, budget)? != edit.expected {
                    return Err(IndexError::Conflict);
                }
                let next = self.store.prepare_resident_roots(
                    &[base],
                    &[edit],
                    StagedMapRoots::prepare(1, budget)?,
                    &token.stage,
                    budget,
                )?;
                token.delta = Some(
                    *next
                        .first()
                        .ok_or_else(|| violation("owner delta root absent"))?,
                );
                Ok(Box::new(token))
            }
            State::Reading | State::Installing | State::Transferred | State::Failed => {
                Err(violation("owner scope cannot prepare another map"))
            }
        }
    }
}

impl ImmutableObjectPort for OwnerScope {
    fn identify(
        &self,
        payload: &[u8],
        children: &[ObjectRef],
        budget: &mut WorkBudget,
    ) -> Result<RegisteredObject, IndexError> {
        self.store.identify(payload, children, budget)
    }
    fn put(
        &self,
        payload: &[u8],
        children: &[ObjectRef],
        budget: &mut WorkBudget,
    ) -> Result<RegisteredObject, IndexError> {
        let state = self
            .state
            .lock()
            .map_err(|_error| violation("owner scope lock poisoned"))?;
        if !matches!(*state, State::Installed) {
            return Err(violation("owner scope cannot put without managed sink"));
        }
        drop(state);
        self.store.put(payload, children, budget)
    }
    fn install(
        &self,
        payload: &[u8],
        children: &[ObjectRef],
        sink: &dyn ObjectWriteSink,
        budget: &mut WorkBudget,
    ) -> Result<RegisteredObject, IndexError> {
        self.flush(sink, budget)?;
        self.store.install(payload, children, sink, budget)
    }
    fn install_batch(
        &self,
        inputs: &[ObjectBatchInput],
        sink: &dyn ObjectWriteSink,
        budget: &mut WorkBudget,
    ) -> Result<RegisteredObjectBatch, IndexError> {
        self.flush(sink, budget)?;
        self.store.install_batch(inputs, sink, budget)
    }
    fn get(
        &self,
        reference: ObjectRef,
        budget: &mut WorkBudget,
    ) -> Result<StoredObject, IndexError> {
        if let Some(object) = self.read_prepared(reference, budget)? {
            return Ok(object);
        }
        self.store.get(reference, budget)
    }
    fn read_retained(
        &self,
        object: &StoredObject,
        budget: &mut WorkBudget,
    ) -> Result<StoredObject, IndexError> {
        self.store.read_retained(object, budget)
    }
}

impl PersistentMapPort for OwnerScope {
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
                self.read_node(reference, budget)
            })
    }
    fn lookup_batch(
        &self,
        root: MapRoot,
        keys: &[&[u8]],
        values: &mut [Option<MapValue>],
        budget: &mut WorkBudget,
    ) -> Result<(), IndexError> {
        if keys.len() > MAX_MAP_LOOKUP_BATCH || keys.len() != values.len() {
            return Err(IndexError::InvalidInput(
                "map lookup batch shape exceeds bound".into(),
            ));
        }
        budget.charge(quanta_index_core::index::WorkUsage {
            bytes: u64::try_from(
                keys.len()
                    .checked_mul(std::mem::size_of::<Option<MapValue>>())
                    .ok_or(IndexError::ResourceLimited)?,
            )
            .map_err(|_error| IndexError::ResourceLimited)?,
            ..quanta_index_core::index::WorkUsage::default()
        })?;
        let mut lookup = original_lookup::OriginalLookup::new();
        for (key, value) in keys.iter().zip(values) {
            charge_key_allocation(key, budget)?;
            let nibbles = key_nibbles(key, budget)?;
            *value = lookup.lookup_with(root, &nibbles, budget, |reference, budget| {
                self.read_node(reference, budget)
            })?;
        }
        Ok(())
    }
    fn scan(
        &self,
        root: MapRoot,
        after: Option<&[u8]>,
        limit: u32,
        budget: &mut WorkBudget,
    ) -> Result<MapPage, IndexError> {
        self.store
            .scan_with(root, after, limit, budget, |reference, budget| {
                self.read_node(reference, budget)
            })
    }
    fn visit_diff(
        &self,
        before: MapRoot,
        after: MapRoot,
        range: MapDiffRange<'_>,
        visitor: &mut dyn MapDiffVisitor,
        budget: &mut WorkBudget,
    ) -> Result<bool, IndexError> {
        super::diff::visit_with(
            before,
            after,
            range,
            visitor,
            budget,
            |reference, budget| self.read_node(reference, budget),
        )
    }
    fn apply(
        &self,
        base: MapRoot,
        edits: &[MapEdit],
        sink: &dyn ObjectWriteSink,
        budget: &mut WorkBudget,
    ) -> Result<MapWrite, IndexError> {
        self.flush(sink, budget)?;
        self.store.apply(base, edits, sink, budget)
    }
    fn apply_batch(
        &self,
        requests: &[MapBatchRequest],
        sink: &dyn ObjectWriteSink,
        budget: &mut WorkBudget,
    ) -> Result<MapBatchWrite, IndexError> {
        self.flush(sink, budget)?;
        self.store.apply_batch(requests, sink, budget)
    }
    fn apply_stream(
        &self,
        bases: &[MapRoot],
        edits: &mut dyn MapEditCursor,
        sink: &dyn ObjectWriteSink,
        budget: &mut WorkBudget,
    ) -> Result<StagedMapRoots, IndexError> {
        self.flush(sink, budget)?;
        self.store.apply_stream(bases, edits, sink, budget)
    }
}
