//! Small metadata and COW roots use the same opaque, child-first install walk.

use std::sync::Arc;

use quanta_index_core::index::{
    AttemptRef, IndexError, MapEdit, MapRoot, MemoryLease, MemoryLifetime, ObjectBatchInput,
    ObjectInstallScope, ObjectRef, StagedMapRoots, StagedObjectReferences, WorkBudget,
    ports::{
        MAX_OBJECT_MAP_BATCH_OBJECTS, MAX_OBJECT_MAP_BATCH_ROOTS, ObjectMapBatchPort,
        ObjectWriteSink, PersistentMapPort, PreparedObjectMapBatch, PreparedOwnerMapScope,
    },
};

use super::{FileObjectStore, MapBuffer, ResidentStage};
use crate::object::MAX_BATCH_BYTES;

struct OriginalPreparedMap {
    store: Arc<FileObjectStore>,
    staged: ResidentStage,
    root: MapRoot,
    attempt: AttemptRef,
    // The box and its retained original stage outlive prepare_one_map.
    _memory: Option<MemoryLease>,
}

impl PreparedObjectMapBatch for OriginalPreparedMap {
    fn root(&self) -> MapRoot {
        self.root
    }

    fn install(
        self: Box<Self>,
        objects: &[ObjectBatchInput],
        additional: Option<(MapRoot, &MapEdit)>,
        attempt: AttemptRef,
        sink: &dyn ObjectWriteSink,
        budget: &mut WorkBudget,
    ) -> Result<(StagedObjectReferences, StagedMapRoots), IndexError> {
        budget.checkpoint()?;
        if attempt != self.attempt
            || sink.managed_scope() != Some(ObjectInstallScope::Attempt(attempt))
        {
            return Err(IndexError::BackendViolation(
                "prepared map attempt or sink scope differs".into(),
            ));
        }
        if objects.len() > MAX_OBJECT_MAP_BATCH_OBJECTS {
            return Err(IndexError::ResourceLimited);
        }
        let count = objects.len();
        let reference_bytes = count
            .checked_mul(std::mem::size_of::<ObjectRef>())
            .ok_or(IndexError::ResourceLimited)?;
        let memory = if reference_bytes == 0 {
            budget.checkpoint()?;
            None
        } else {
            budget.reserve_memory(
                u64::try_from(reference_bytes).map_err(|_width| IndexError::ResourceLimited)?,
                MemoryLifetime::Retained,
            )?
        };
        let mut references = Vec::new();
        references
            .try_reserve_exact(count)
            .map_err(|_allocation| IndexError::ResourceLimited)?;
        let mut prepared = MapBuffer::new(MAX_OBJECT_MAP_BATCH_OBJECTS);
        let mut wire_bytes = 0_usize;
        for input in objects {
            let object = self
                .store
                .prepare_object(&input.payload, &input.children, budget)?;
            wire_bytes = wire_bytes
                .checked_add(object.wire.len())
                .ok_or(IndexError::ResourceLimited)?;
            references.push(object.registration.reference);
            prepared.push(object, budget)?;
        }
        if u64::try_from(wire_bytes).map_err(|_width| IndexError::ResourceLimited)?
            > MAX_BATCH_BYTES
        {
            return Err(IndexError::ResourceLimited);
        }
        let objects = StagedObjectReferences::from_parts(references, memory)?;
        let OriginalPreparedMap {
            store,
            mut staged,
            root,
            attempt: _attempt,
            _memory,
        } = *self;
        let roots = if let Some((base, edit)) = additional {
            // The second CAS reads its original base, never the provisional
            // first root. No object is inserted or reserved before both pass.
            if store.lookup(base, &edit.key, budget)? != edit.expected {
                return Err(IndexError::Conflict);
            }
            let next = store.prepare_resident_roots(
                &[base],
                &[edit],
                StagedMapRoots::prepare(1, budget)?,
                &staged,
                budget,
            )?;
            let additional_root = *next
                .first()
                .ok_or_else(|| IndexError::BackendViolation("additional map root absent".into()))?;
            drop(next);
            let mut roots = StagedMapRoots::prepare(2, budget)?;
            roots.push(root)?;
            roots.push(additional_root)?;
            roots.finish()?
        } else {
            let mut roots = StagedMapRoots::prepare(1, budget)?;
            roots.push(root)?;
            roots.finish()?
        };
        staged.admit_object_wires(wire_bytes)?;
        while let Some(object) = prepared.pop() {
            let _reference = staged.insert_object(object, budget)?;
        }
        store.install_reachable_objects(roots.as_slice(), &objects, &staged, sink, budget)?;
        budget.checkpoint()?;
        Ok((objects, roots))
    }
}

impl ObjectMapBatchPort for FileObjectStore {
    fn prepare_owner_maps(
        self: Arc<Self>,
        objects: &[ObjectBatchInput],
        bases: &[MapRoot],
        edits: &[&MapEdit],
        attempt: AttemptRef,
        budget: &mut WorkBudget,
    ) -> Result<Option<Arc<dyn PreparedOwnerMapScope>>, IndexError> {
        super::owner_scope::OwnerScope::prepare(self, objects, bases, edits, attempt, budget)
            .map(Some)
    }

    fn prepare_one_map(
        self: Arc<Self>,
        base: MapRoot,
        edit: &MapEdit,
        attempt: AttemptRef,
        budget: &mut WorkBudget,
    ) -> Result<Box<dyn PreparedObjectMapBatch>, IndexError> {
        budget.checkpoint()?;
        let staged = ResidentStage::with_objects(MAX_OBJECT_MAP_BATCH_OBJECTS, 0)?;
        let roots = self.prepare_resident_roots(
            &[base],
            &[edit],
            StagedMapRoots::prepare(1, budget)?,
            &staged,
            budget,
        )?;
        let root = *roots
            .first()
            .ok_or_else(|| IndexError::BackendViolation("prepared map root absent".into()))?;
        drop(roots);
        let memory = budget.reserve_memory(
            u64::try_from(std::mem::size_of::<OriginalPreparedMap>())
                .map_err(|_width| IndexError::ResourceLimited)?,
            MemoryLifetime::Retained,
        )?;
        Ok(Box::new(OriginalPreparedMap {
            store: self,
            staged,
            root,
            attempt,
            _memory: memory,
        }))
    }

    fn install_objects_and_maps(
        &self,
        inputs: &[ObjectBatchInput],
        bases: &[MapRoot],
        edits: &[&MapEdit],
        sink: &dyn ObjectWriteSink,
        budget: &mut WorkBudget,
    ) -> Result<(StagedObjectReferences, StagedMapRoots), IndexError> {
        budget.checkpoint()?;
        if inputs.is_empty()
            || inputs.len() > MAX_OBJECT_MAP_BATCH_OBJECTS
            || bases.is_empty()
            || bases.len() > MAX_OBJECT_MAP_BATCH_ROOTS
            || edits.len() != bases.len()
        {
            return Err(IndexError::ResourceLimited);
        }
        let bytes = inputs
            .len()
            .checked_mul(std::mem::size_of::<ObjectRef>())
            .ok_or(IndexError::ResourceLimited)?;
        let memory = budget.reserve_memory(
            u64::try_from(bytes).map_err(|_width| IndexError::ResourceLimited)?,
            MemoryLifetime::Retained,
        )?;
        let mut references = Vec::new();
        references
            .try_reserve_exact(inputs.len())
            .map_err(|_allocation| IndexError::ResourceLimited)?;
        let mut prepared = MapBuffer::new(MAX_OBJECT_MAP_BATCH_OBJECTS);
        let mut wire_bytes = 0_usize;
        for input in inputs {
            let object = self.prepare_object(&input.payload, &input.children, budget)?;
            wire_bytes = wire_bytes
                .checked_add(object.wire.len())
                .ok_or(IndexError::ResourceLimited)?;
            references.push(object.registration.reference);
            prepared.push(object, budget)?;
        }
        let objects = StagedObjectReferences::from_parts(references, memory)?;
        let staged = ResidentStage::with_objects(inputs.len(), wire_bytes)?;
        let roots = self.prepare_resident_roots(
            bases,
            edits,
            StagedMapRoots::prepare(bases.len(), budget)?,
            &staged,
            budget,
        )?;
        // All original-base preconditions passed. Only now add opaque DATA;
        // COW reads above never interpret it as a radix node.
        while let Some(object) = prepared.pop() {
            let _reference = staged.insert_object(object, budget)?;
        }
        self.install_reachable_objects(roots.as_slice(), &objects, &staged, sink, budget)?;
        budget.checkpoint()?;
        Ok((objects, roots))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use quanta_index_core::{
        RequestBudgetV1,
        index::{
            AttemptId, CollectionId, MapBatchRequest, OperationKey, ProducerId, RegisteredObject,
            SlotId, StreamId, TenantId, WorkLimits, WorkspaceKey, WorktreeId,
            ports::{ImmutableObjectPort, OwnerMapCompletion, PersistentMapPort},
        },
    };
    use std::{
        sync::atomic::{AtomicUsize, Ordering},
        time::Duration,
    };

    type TestResult = Result<(), Box<dyn std::error::Error>>;

    #[derive(Default)]
    struct Sink {
        groups: AtomicUsize,
        scope: Option<AttemptRef>,
    }
    impl ObjectWriteSink for Sink {
        fn managed_scope(&self) -> Option<ObjectInstallScope> {
            self.scope.map(ObjectInstallScope::Attempt)
        }
        fn reserve(&self, _object: &RegisteredObject) -> Result<(), IndexError> {
            Ok(())
        }
        fn installed(&self, _object: &RegisteredObject) -> Result<(), IndexError> {
            Ok(())
        }
        fn reserve_batch(&self, _objects: &[RegisteredObject]) -> Result<(), IndexError> {
            let _previous = self.groups.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }
        fn installed_batch(&self, _objects: &[RegisteredObject]) -> Result<(), IndexError> {
            Ok(())
        }
    }
    fn budget() -> WorkBudget {
        WorkBudget::with_request(
            WorkLimits {
                reads: 1_000_000,
                writes: 1_000_000,
                bytes: 536_870_912,
                mutations: 1_000_000,
            },
            RequestBudgetV1::for_duration(Duration::from_secs(30)),
        )
    }
    fn attempt() -> AttemptRef {
        AttemptRef {
            operation: OperationKey {
                workspace: WorkspaceKey {
                    tenant: TenantId([1; 16]),
                    collection: CollectionId([2; 16]),
                    worktree: WorktreeId([3; 16]),
                    slot: SlotId([4; 16]),
                },
                epoch: 1,
                producer: ProducerId([5; 16]),
                stream: StreamId([6; 16]),
                stream_epoch: 1,
                sequence: 1,
            },
            attempt: AttemptId([7; 16]),
            fence: 1,
        }
    }

    fn scoped_sink(attempt: AttemptRef) -> Sink {
        Sink {
            groups: AtomicUsize::new(0),
            scope: Some(attempt),
        }
    }

    fn parent_inputs(
        store: &FileObjectStore,
        delta: MapRoot,
    ) -> Result<(Vec<ObjectBatchInput>, [ObjectRef; 2]), IndexError> {
        let delta = delta
            .0
            .ok_or_else(|| IndexError::BackendViolation("prepared delta root absent".into()))?;
        let descriptor = ObjectBatchInput {
            payload: b"delta-descriptor".to_vec(),
            children: vec![delta],
        };
        let descriptor_ref = store
            .identify(&descriptor.payload, &descriptor.children, &mut budget())?
            .reference;
        let parent = ObjectBatchInput {
            payload: b"coverage-parent".to_vec(),
            children: vec![descriptor_ref],
        };
        let parent_ref = store
            .identify(&parent.payload, &parent.children, &mut budget())?
            .reference;
        Ok((vec![descriptor, parent], [descriptor_ref, parent_ref]))
    }
    fn require(value: bool) -> Result<(), IndexError> {
        if value {
            Ok(())
        } else {
            Err(IndexError::BackendViolation(
                "object/map batch oracle mismatch".into(),
            ))
        }
    }
    fn fixture(
        store: &FileObjectStore,
    ) -> Result<(ObjectRef, MapRoot, Vec<ObjectBatchInput>, [ObjectRef; 2]), IndexError> {
        let mut work = budget();
        let old = store.put(b"old-source", &[], &mut work)?.reference;
        let root = store
            .apply(
                MapRoot(None),
                &[MapEdit::objects(b"owner".to_vec(), None, Some(old))],
                &Sink::default(),
                &mut work,
            )?
            .root;
        let member = ObjectBatchInput {
            payload: b"fixed-member".to_vec(),
            children: vec![old],
        };
        let first = store
            .identify(&member.payload, &member.children, &mut work)?
            .reference;
        let coverage = ObjectBatchInput {
            payload: b"fixed-coverage".to_vec(),
            children: vec![first],
        };
        let second = store
            .identify(&coverage.payload, &coverage.children, &mut work)?
            .reference;
        Ok((old, root, vec![member, coverage], [first, second]))
    }

    #[test]
    fn joined_pair_preserves_original_wire_and_roots_with_one_install_group() -> TestResult {
        let home = tempfile::tempdir()?;
        let original = FileObjectStore::open(home.path().join("original"))?;
        let joined = FileObjectStore::open(home.path().join("joined"))?;
        let (old, base, inputs, expected) = fixture(&original)?;
        let (old_joined, base_joined, joined_inputs, joined_expected) = fixture(&joined)?;
        require(old == old_joined && base == base_joined && expected == joined_expected)?;
        let first = MapEdit::objects(b"owner".to_vec(), Some(old), expected.get(1).copied());
        let second = MapEdit::objects(b"owner".to_vec(), Some(old), None);
        let baseline_sink = Sink::default();
        let mut baseline_work = budget();
        let installed = original.install_batch(&inputs, &baseline_sink, &mut baseline_work)?;
        require(installed.iter().map(|object| object.reference).eq(expected))?;
        let baseline = original.apply_batch(
            &[
                MapBatchRequest {
                    base,
                    edits: vec![first.clone()],
                },
                MapBatchRequest {
                    base,
                    edits: vec![second.clone()],
                },
            ],
            &baseline_sink,
            &mut baseline_work,
        )?;
        require(baseline_sink.groups.load(Ordering::SeqCst) == 2)?;
        require(
            joined
                .get(*expected.first().ok_or("member absent")?, &mut budget())
                .is_err(),
        )?;
        let sink = Sink::default();
        let mut joined_work = budget();
        let (actual, roots) = joined.install_objects_and_maps(
            &joined_inputs,
            &[base_joined, base_joined],
            &[&first, &second],
            &sink,
            &mut joined_work,
        )?;
        require(*actual == expected && roots == baseline.roots)?;
        // WorkUsage also includes resident staging/mark operations. Physical
        // group reduction is observed through the sink, not inferred from it.
        require(sink.groups.load(Ordering::SeqCst) == 1)?;
        for reference in expected {
            require(
                joined.get(reference, &mut budget())? == original.get(reference, &mut budget())?,
            )?;
        }
        let root = roots.first().copied().ok_or("owner root absent")?;
        require(joined.lookup_object(root, b"owner", &mut budget())? == expected.get(1).copied())?;
        require(roots.get(1).copied() == Some(MapRoot(None)))?;
        let reference = root.0.ok_or("root reference absent")?;
        require(joined.get(reference, &mut budget())? == original.get(reference, &mut budget())?)?;
        Ok(())
    }

    #[test]
    fn second_precondition_and_cancellation_refuse_before_installing_metadata() -> TestResult {
        let home = tempfile::tempdir()?;
        let store = FileObjectStore::open(home.path().join("store"))?;
        let (old, base, inputs, expected) = fixture(&store)?;
        let first = MapEdit::objects(b"owner".to_vec(), Some(old), expected.get(1).copied());
        let wrong = MapEdit::objects(b"owner".to_vec(), None, None);
        let sink = Sink::default();
        require(matches!(
            store.install_objects_and_maps(
                &inputs,
                &[base, base],
                &[&first, &wrong],
                &sink,
                &mut budget()
            ),
            Err(IndexError::Conflict)
        ))?;
        require(sink.groups.load(Ordering::SeqCst) == 0)?;
        require(
            store
                .get(*expected.first().ok_or("member absent")?, &mut budget())
                .is_err(),
        )?;
        require(store.lookup_object(base, b"owner", &mut budget())? == Some(old))?;
        let mut cancelled = budget();
        cancelled.request().cancel_handle().cancel();
        require(matches!(
            store.install_objects_and_maps(&inputs, &[base], &[&first], &sink, &mut cancelled),
            Err(IndexError::Cancelled)
        ))?;
        require(sink.groups.load(Ordering::SeqCst) == 0)?;
        Ok(())
    }

    #[test]
    fn duplicate_and_unreferenced_object_slots_install_once_on_noop_root() -> TestResult {
        let home = tempfile::tempdir()?;
        let store = FileObjectStore::open(home.path().join("store"))?;
        let input = ObjectBatchInput {
            payload: b"data-only".to_vec(),
            children: Vec::new(),
        };
        let reference = store
            .identify(&input.payload, &[], &mut budget())?
            .reference;
        let edit = MapEdit::objects(b"absent".to_vec(), None, None);
        let sink = Sink::default();
        let (actual, roots) = store.install_objects_and_maps(
            &[input.clone(), input],
            &[MapRoot(None)],
            &[&edit],
            &sink,
            &mut budget(),
        )?;
        require(*actual == [reference, reference] && *roots == [MapRoot(None)])?;
        require(sink.groups.load(Ordering::SeqCst) == 1)?;
        require(store.get(reference, &mut budget())?.payload.as_slice() == b"data-only")?;
        Ok(())
    }

    #[test]
    fn memory_bounds_and_reserve_failure_preserve_original_refusals() -> TestResult {
        use quanta_index_core::index::{MemoryLease, MemoryScope, MemoryScopePort};
        use std::sync::Arc;
        struct Deny;
        impl MemoryScopePort for Deny {
            fn reserve(
                &self,
                _bytes: u64,
                _lifetime: MemoryLifetime,
            ) -> Result<MemoryLease, IndexError> {
                Err(IndexError::ResourceLimited)
            }
        }
        struct Refuse;
        impl ObjectWriteSink for Refuse {
            fn reserve(&self, _object: &RegisteredObject) -> Result<(), IndexError> {
                Ok(())
            }
            fn installed(&self, _object: &RegisteredObject) -> Result<(), IndexError> {
                Ok(())
            }
            fn reserve_batch(&self, _objects: &[RegisteredObject]) -> Result<(), IndexError> {
                Err(IndexError::Storage("original reserve refusal".into()))
            }
            fn installed_batch(&self, _objects: &[RegisteredObject]) -> Result<(), IndexError> {
                Err(IndexError::BackendViolation(
                    "confirmation followed a refused reserve".into(),
                ))
            }
        }
        let home = tempfile::tempdir()?;
        let store = FileObjectStore::open(home.path().join("store"))?;
        let (old, base, inputs, expected) = fixture(&store)?;
        let edit = MapEdit::objects(b"owner".to_vec(), Some(old), expected.get(1).copied());
        let sink = Sink::default();
        let mut denied = budget();
        denied.bind_memory_scope(Some(MemoryScope::new(Arc::new(Deny))));
        require(matches!(
            store.install_objects_and_maps(&inputs, &[base], &[&edit], &sink, &mut denied),
            Err(IndexError::ResourceLimited)
        ))?;
        let long = MapEdit::objects(vec![0; 65], None, expected.get(1).copied());
        require(matches!(
            store.install_objects_and_maps(&inputs, &[base], &[&long], &sink, &mut budget()),
            Err(IndexError::ResourceLimited)
        ))?;
        require(sink.groups.load(Ordering::SeqCst) == 0)?;
        require(
            matches!(store.install_objects_and_maps(&inputs, &[base], &[&edit], &Refuse, &mut budget()),
            Err(IndexError::Storage(message)) if message == "original reserve refusal"),
        )?;
        require(
            store
                .get(*expected.first().ok_or("member absent")?, &mut budget())
                .is_err(),
        )?;
        require(store.lookup_object(base, b"owner", &mut budget())? == Some(old))?;
        Ok(())
    }

    #[test]
    fn owner_scope_reads_pending_wires_and_matches_original_four_root_install() -> TestResult {
        use quanta_index_core::index::ports::{MapDiffRange, MapDiffVisitor};

        struct Changes(usize);
        impl MapDiffVisitor for Changes {
            fn changed(
                &mut self,
                _key: &[u8],
                _before: Option<quanta_index_core::index::MapValue>,
                _after: Option<quanta_index_core::index::MapValue>,
                _budget: &mut WorkBudget,
            ) -> Result<(), IndexError> {
                self.0 = self.0.checked_add(1).ok_or(IndexError::ResourceLimited)?;
                Ok(())
            }
        }

        let home = tempfile::tempdir()?;
        let baseline = Arc::new(FileObjectStore::open(home.path().join("baseline"))?);
        let joined = Arc::new(FileObjectStore::open(home.path().join("joined"))?);
        let (old, base, inputs, expected) = fixture(&baseline)?;
        let (joined_old, joined_base, joined_inputs, joined_expected) = fixture(&joined)?;
        require(old == joined_old && base == joined_base && expected == joined_expected)?;
        let [member_ref, owner_ref] = expected;
        let owner = MapEdit::objects(b"owner".to_vec(), Some(old), Some(owner_ref));
        let missing = MapEdit::objects(b"owner".to_vec(), Some(old), None);
        let baseline_sink = scoped_sink(attempt());
        let (baseline_refs, baseline_roots) = baseline.install_objects_and_maps(
            &inputs,
            &[base, base],
            &[&owner, &missing],
            &baseline_sink,
            &mut budget(),
        )?;
        require(*baseline_refs == expected && baseline_roots.len() == 2)?;

        let joined_sink = scoped_sink(attempt());
        let scope = Arc::clone(&joined)
            .prepare_owner_maps(
                &joined_inputs,
                &[joined_base, joined_base],
                &[&owner, &missing],
                attempt(),
                &mut budget(),
            )?
            .ok_or("original owner scope absent")?;
        require(scope.references() == expected && scope.roots() == baseline_roots.as_slice())?;
        require(joined_sink.groups.load(Ordering::SeqCst) == 0)?;
        require(joined.get(member_ref, &mut budget()).is_err())?;
        require(
            scope.objects().get(member_ref, &mut budget())?.payload
                == inputs.first().ok_or("member input absent")?.payload,
        )?;
        let owners = *scope.roots().first().ok_or("owners root absent")?;
        require(
            scope
                .maps()
                .lookup_object(owners, b"owner", &mut budget())?
                == Some(owner_ref),
        )?;
        let mut values = [None];
        scope
            .maps()
            .lookup_batch(owners, &[b"owner".as_slice()], &mut values, &mut budget())?;
        require(values.first().copied() == Some(Some(owner_ref.into())))?;
        let page = scope.maps().scan(owners, None, 1, &mut budget())?;
        require(
            page.entries.len() == 1
                && page
                    .entries
                    .first()
                    .is_some_and(|entry| entry.0 == b"owner"),
        )?;
        let mut changes = Changes(0);
        require(
            !scope.maps().visit_diff(
                joined_base,
                owners,
                MapDiffRange {
                    prefix: b"",
                    after: None,
                    limit: 2,
                },
                &mut changes,
                &mut budget(),
            )? && changes.0 == 1,
        )?;

        let delta = MapEdit::objects(b"delta".to_vec(), None, Some(member_ref));
        let baseline_token = Arc::clone(&baseline).prepare_one_map(
            MapRoot(None),
            &delta,
            attempt(),
            &mut budget(),
        )?;
        let baseline_delta = baseline_token.root().0.ok_or("baseline delta absent")?;
        let joined_token =
            scope
                .writer()
                .prepare_one_map(MapRoot(None), &delta, attempt(), &mut budget())?;
        let joined_delta = joined_token.root().0.ok_or("joined delta absent")?;
        require(baseline_delta == joined_delta)?;
        let descriptor = ObjectBatchInput {
            payload: b"fixed-directory".to_vec(),
            children: vec![baseline_delta],
        };
        let descriptor_ref = baseline
            .identify(&descriptor.payload, &descriptor.children, &mut budget())?
            .reference;
        let parent = ObjectBatchInput {
            payload: b"fixed-parent".to_vec(),
            children: vec![
                descriptor_ref,
                baseline_roots
                    .first()
                    .ok_or("owner root absent")?
                    .0
                    .ok_or("owner root absent")?,
            ],
        };
        let parent_ref = baseline
            .identify(&parent.payload, &parent.children, &mut budget())?
            .reference;
        let coverage = MapEdit::objects(b"coverage".to_vec(), None, Some(parent_ref));
        let (baseline_tail_refs, baseline_tail_roots) = baseline_token.install(
            &[descriptor.clone(), parent.clone()],
            Some((MapRoot(None), &coverage)),
            attempt(),
            &baseline_sink,
            &mut budget(),
        )?;
        let (actual, roots) = joined_token.install(
            &[descriptor, parent],
            Some((MapRoot(None), &coverage)),
            attempt(),
            &joined_sink,
            &mut budget(),
        )?;
        require(*actual == [member_ref, owner_ref, descriptor_ref, parent_ref])?;
        require(roots.as_slice().get(..2) == Some(baseline_roots.as_slice()))?;
        require(roots.as_slice().get(2..) == Some(baseline_tail_roots.as_slice()))?;
        require(*baseline_tail_refs == [descriptor_ref, parent_ref])?;
        require(baseline_sink.groups.load(Ordering::SeqCst) == 2)?;
        require(joined_sink.groups.load(Ordering::SeqCst) == 1)?;
        for reference in &*actual {
            require(
                joined.get(*reference, &mut budget())?
                    == baseline.get(*reference, &mut budget())?,
            )?;
        }
        Ok(())
    }

    #[test]
    fn owner_scope_first_write_flushes_and_wrong_final_cas_installs_nothing() -> TestResult {
        let home = tempfile::tempdir()?;
        let store = Arc::new(FileObjectStore::open(home.path().join("store"))?);
        let (old, base, inputs, expected) = fixture(&store)?;
        let [member_ref, owner_ref] = expected;
        let owner = MapEdit::objects(b"owner".to_vec(), Some(old), Some(owner_ref));
        let missing = MapEdit::objects(b"owner".to_vec(), Some(old), None);
        let scope = Arc::clone(&store)
            .prepare_owner_maps(
                &inputs,
                &[base, base],
                &[&owner, &missing],
                attempt(),
                &mut budget(),
            )?
            .ok_or("owner scope absent")?;
        let sink = scoped_sink(attempt());
        let extra = MapEdit::objects(b"extra".to_vec(), None, Some(member_ref));
        let _root = scope
            .maps()
            .apply(MapRoot(None), &[extra], &sink, &mut budget())?
            .root;
        require(sink.groups.load(Ordering::SeqCst) == 2)?;
        require(matches!(
            scope.take_prepared(attempt(), &mut budget())?,
            OwnerMapCompletion::Installed
        ))?;
        require(
            store.get(member_ref, &mut budget())?.payload
                == inputs.first().ok_or("member input absent")?.payload,
        )?;

        let second = Arc::new(FileObjectStore::open(home.path().join("second"))?);
        let (old2, base2, inputs2, expected2) = fixture(&second)?;
        require(old2 == old && base2 == base && expected2 == expected)?;
        let pending = Arc::clone(&second)
            .prepare_owner_maps(
                &inputs2,
                &[base2, base2],
                &[&owner, &missing],
                attempt(),
                &mut budget(),
            )?
            .ok_or("owner scope absent")?;
        let other_attempt = AttemptRef {
            fence: 2,
            ..attempt()
        };
        require(pending.take_prepared(other_attempt, &mut budget()).is_err())?;
        let provisional = *pending.roots().first().ok_or("provisional root absent")?;
        let token = match pending.take_prepared(attempt(), &mut budget())? {
            OwnerMapCompletion::Prepared(token) => token,
            OwnerMapCompletion::Transferred | OwnerMapCompletion::Installed => {
                return Err("pending owner token absent".into());
            }
        };
        require(matches!(
            pending.take_prepared(attempt(), &mut budget())?,
            OwnerMapCompletion::Transferred
        ))?;
        let wrong = MapEdit::objects(b"owner".to_vec(), Some(owner_ref), None);
        let refused_sink = scoped_sink(attempt());
        require(matches!(
            token.install(
                &[],
                Some((provisional, &wrong)),
                attempt(),
                &refused_sink,
                &mut budget()
            ),
            Err(IndexError::CorruptObject(_))
        ))?;
        require(refused_sink.groups.load(Ordering::SeqCst) == 0)?;
        require(second.get(member_ref, &mut budget()).is_err())?;
        require(second.lookup_object(base2, b"owner", &mut budget())? == Some(old2))?;
        Ok(())
    }

    #[test]
    fn owner_scope_drop_and_cancel_leave_no_physical_metadata() -> TestResult {
        let home = tempfile::tempdir()?;
        let store = Arc::new(FileObjectStore::open(home.path().join("store"))?);
        let (old, base, inputs, expected) = fixture(&store)?;
        let [member_ref, owner_ref] = expected;
        let owner = MapEdit::objects(b"owner".to_vec(), Some(old), Some(owner_ref));
        let missing = MapEdit::objects(b"owner".to_vec(), Some(old), None);
        let scope = Arc::clone(&store)
            .prepare_owner_maps(
                &inputs,
                &[base, base],
                &[&owner, &missing],
                attempt(),
                &mut budget(),
            )?
            .ok_or("owner scope absent")?;
        let sink = scoped_sink(attempt());
        let mut cancelled = budget();
        cancelled.request().cancel_handle().cancel();
        require(matches!(
            scope.maps().apply(
                MapRoot(None),
                &[MapEdit::objects(b"extra".to_vec(), None, Some(member_ref))],
                &sink,
                &mut cancelled
            ),
            Err(IndexError::Cancelled)
        ))?;
        require(sink.groups.load(Ordering::SeqCst) == 0)?;
        drop(scope);
        require(store.get(member_ref, &mut budget()).is_err())?;
        require(store.lookup_object(base, b"owner", &mut budget())? == Some(old))?;
        Ok(())
    }

    #[test]
    fn prepared_delta_and_coverage_match_original_roots_in_one_custody_group() -> TestResult {
        let home = tempfile::tempdir()?;
        let baseline = FileObjectStore::open(home.path().join("baseline"))?;
        let joined = Arc::new(FileObjectStore::open(home.path().join("joined"))?);
        let (old, base, _inputs, _expected) = fixture(&baseline)?;
        let (joined_old, joined_base, _inputs, _expected) = fixture(&joined)?;
        require(old == joined_old && base == joined_base)?;
        let replacement = baseline.put(b"replacement", &[], &mut budget())?.reference;
        let joined_replacement = joined.put(b"replacement", &[], &mut budget())?.reference;
        require(replacement == joined_replacement)?;
        let delta_edit = MapEdit::objects(b"owner".to_vec(), Some(old), Some(replacement));
        let baseline_sink = Sink::default();
        let baseline_delta = baseline
            .apply(
                base,
                std::slice::from_ref(&delta_edit),
                &baseline_sink,
                &mut budget(),
            )?
            .root;
        let (baseline_inputs, expected) = parent_inputs(&baseline, baseline_delta)?;
        let coverage_edit =
            MapEdit::objects(b"owner".to_vec(), Some(old), expected.get(1).copied());
        let (baseline_refs, baseline_roots) = baseline.install_objects_and_maps(
            &baseline_inputs,
            &[base],
            &[&coverage_edit],
            &baseline_sink,
            &mut budget(),
        )?;
        require(*baseline_refs == expected && baseline_roots.len() == 1)?;
        require(baseline_sink.groups.load(Ordering::SeqCst) == 2)?;

        let attempt = attempt();
        let token = Arc::clone(&joined).prepare_one_map(
            joined_base,
            &delta_edit,
            attempt,
            &mut budget(),
        )?;
        require(token.root() == baseline_delta)?;
        let delta_reference = token.root().0.ok_or("missing delta root")?;
        require(joined.get(delta_reference, &mut budget()).is_err())?;
        let (joined_inputs, joined_expected) = parent_inputs(&joined, token.root())?;
        require(joined_expected == expected)?;
        let sink = scoped_sink(attempt);
        let (actual, roots) = token.install(
            &joined_inputs,
            Some((joined_base, &coverage_edit)),
            attempt,
            &sink,
            &mut budget(),
        )?;
        require(*actual == expected)?;
        require(
            roots.as_slice()
                == [
                    baseline_delta,
                    *baseline_roots
                        .first()
                        .ok_or("missing baseline coverage root")?,
                ],
        )?;
        require(sink.groups.load(Ordering::SeqCst) == 1)?;
        for reference in [
            delta_reference,
            *expected.first().ok_or("missing descriptor")?,
            *expected.get(1).ok_or("missing parent")?,
        ] {
            require(
                joined.get(reference, &mut budget())? == baseline.get(reference, &mut budget())?,
            )?;
        }
        Ok(())
    }

    #[test]
    fn prepared_delta_rejects_foreign_attempt_scope_and_second_cas_before_io() -> TestResult {
        let home = tempfile::tempdir()?;
        let store = Arc::new(FileObjectStore::open(home.path().join("store"))?);
        let (old, base, _inputs, _expected) = fixture(&store)?;
        let replacement = store.put(b"replacement", &[], &mut budget())?.reference;
        let delta_edit = MapEdit::objects(b"owner".to_vec(), Some(old), Some(replacement));
        let attempt = attempt();
        let sink = scoped_sink(attempt);
        let token =
            Arc::clone(&store).prepare_one_map(base, &delta_edit, attempt, &mut budget())?;
        let (inputs, expected) = parent_inputs(&store, token.root())?;
        let wrong = MapEdit::objects(b"owner".to_vec(), None, expected.get(1).copied());
        require(matches!(
            token.install(&inputs, Some((base, &wrong)), attempt, &sink, &mut budget()),
            Err(IndexError::Conflict)
        ))?;
        require(sink.groups.load(Ordering::SeqCst) == 0)?;
        require(
            store
                .get(
                    *expected.first().ok_or("missing descriptor")?,
                    &mut budget(),
                )
                .is_err(),
        )?;

        let token =
            Arc::clone(&store).prepare_one_map(base, &delta_edit, attempt, &mut budget())?;
        let provisional = token.root();
        require(
            store
                .get(
                    provisional.0.ok_or("missing provisional root")?,
                    &mut budget(),
                )
                .is_err(),
        )?;
        let from_provisional = MapEdit::objects(b"owner".to_vec(), Some(replacement), None);
        require(matches!(
            token.install(
                &[],
                Some((provisional, &from_provisional)),
                attempt,
                &sink,
                &mut budget(),
            ),
            Err(IndexError::CorruptObject(message)) if message == "referenced object is absent"
        ))?;
        require(sink.groups.load(Ordering::SeqCst) == 0)?;

        let token =
            Arc::clone(&store).prepare_one_map(base, &delta_edit, attempt, &mut budget())?;
        let foreign = AttemptRef {
            fence: attempt
                .fence
                .checked_add(1)
                .ok_or(IndexError::ResourceLimited)?,
            ..attempt
        };
        require(matches!(
            token.install(&inputs, None, foreign, &sink, &mut budget()),
            Err(IndexError::BackendViolation(_))
        ))?;
        let token =
            Arc::clone(&store).prepare_one_map(base, &delta_edit, attempt, &mut budget())?;
        require(matches!(
            token.install(&inputs, None, attempt, &Sink::default(), &mut budget()),
            Err(IndexError::BackendViolation(_))
        ))?;
        let token =
            Arc::clone(&store).prepare_one_map(base, &delta_edit, attempt, &mut budget())?;
        let mut cancelled = budget();
        cancelled.request().cancel_handle().cancel();
        require(matches!(
            token.install(&inputs, None, attempt, &sink, &mut cancelled),
            Err(IndexError::Cancelled)
        ))?;
        let token =
            Arc::clone(&store).prepare_one_map(base, &delta_edit, attempt, &mut budget())?;
        require(matches!(
            token.install(
                &[
                    inputs.first().ok_or("missing descriptor input")?.clone(),
                    inputs.get(1).ok_or("missing parent input")?.clone(),
                    inputs.first().ok_or("missing descriptor input")?.clone()
                ],
                None,
                attempt,
                &sink,
                &mut budget()
            ),
            Err(IndexError::ResourceLimited)
        ))?;
        require(sink.groups.load(Ordering::SeqCst) == 0)?;
        require(
            store
                .get(
                    *expected.first().ok_or("missing descriptor")?,
                    &mut budget(),
                )
                .is_err(),
        )?;
        let too_long = MapEdit::objects(vec![0; 65], None, Some(replacement));
        require(matches!(
            Arc::clone(&store).prepare_one_map(base, &too_long, attempt, &mut budget()),
            Err(IndexError::ResourceLimited)
        ))?;
        Ok(())
    }

    #[test]
    fn prepared_delta_accepts_zero_objects_and_installs_original_roots() -> TestResult {
        let home = tempfile::tempdir()?;
        let store = Arc::new(FileObjectStore::open(home.path().join("store"))?);
        let (old, base, _inputs, _expected) = fixture(&store)?;
        let replacement = store.put(b"replacement", &[], &mut budget())?.reference;
        let delta_edit = MapEdit::objects(b"owner".to_vec(), Some(old), Some(replacement));
        let coverage_edit = MapEdit::objects(b"owner".to_vec(), Some(old), None);
        let attempt = attempt();
        let token =
            Arc::clone(&store).prepare_one_map(base, &delta_edit, attempt, &mut budget())?;
        let delta = token.root();
        let sink = scoped_sink(attempt);
        let (references, roots) = token.install(
            &[],
            Some((base, &coverage_edit)),
            attempt,
            &sink,
            &mut budget(),
        )?;
        require(references.is_empty() && roots.as_slice() == [delta, MapRoot(None)])?;
        require(sink.groups.load(Ordering::SeqCst) == 1)?;
        require(store.lookup_object(delta, b"owner", &mut budget())? == Some(replacement))?;
        Ok(())
    }

    #[test]
    fn prepared_delta_preserves_reserve_and_confirmation_failures() -> TestResult {
        struct FailSink {
            attempt: AttemptRef,
            confirm: bool,
        }
        impl ObjectWriteSink for FailSink {
            fn managed_scope(&self) -> Option<ObjectInstallScope> {
                Some(ObjectInstallScope::Attempt(self.attempt))
            }
            fn reserve(&self, _object: &RegisteredObject) -> Result<(), IndexError> {
                Err(IndexError::Storage("unexpected scalar reserve".into()))
            }
            fn installed(&self, _object: &RegisteredObject) -> Result<(), IndexError> {
                Err(IndexError::Storage("unexpected scalar confirmation".into()))
            }
            fn reserve_batch(&self, _objects: &[RegisteredObject]) -> Result<(), IndexError> {
                if self.confirm {
                    Ok(())
                } else {
                    Err(IndexError::Storage("prepared reserve refusal".into()))
                }
            }
            fn installed_batch(&self, _objects: &[RegisteredObject]) -> Result<(), IndexError> {
                Err(IndexError::Storage("prepared confirmation refusal".into()))
            }
        }
        let home = tempfile::tempdir()?;
        let store = Arc::new(FileObjectStore::open(home.path().join("store"))?);
        let (old, base, _inputs, _expected) = fixture(&store)?;
        let replacement = store.put(b"replacement", &[], &mut budget())?.reference;
        let edit = MapEdit::objects(b"owner".to_vec(), Some(old), Some(replacement));
        let attempt = attempt();
        let reserve_token =
            Arc::clone(&store).prepare_one_map(base, &edit, attempt, &mut budget())?;
        let reserve_root = reserve_token.root();
        require(matches!(
            reserve_token.install(
                &[],
                None,
                attempt,
                &FailSink { attempt, confirm: false },
                &mut budget(),
            ),
            Err(IndexError::Storage(message)) if message == "prepared reserve refusal"
        ))?;
        require(
            store
                .get(reserve_root.0.ok_or("missing reserve root")?, &mut budget())
                .is_err(),
        )?;
        let confirm_token =
            Arc::clone(&store).prepare_one_map(base, &edit, attempt, &mut budget())?;
        require(matches!(
            confirm_token.install(
                &[],
                None,
                attempt,
                &FailSink { attempt, confirm: true },
                &mut budget(),
            ),
            Err(IndexError::Storage(message)) if message == "prepared confirmation refusal"
        ))?;
        Ok(())
    }

    #[test]
    fn returned_roots_and_references_keep_data_custody_until_last_drop() -> TestResult {
        use quanta_index_core::index::{
            MemoryLease, MemoryLeaseOwner, MemoryScope, MemoryScopePort,
        };
        use std::sync::{Arc, atomic::AtomicU64};
        struct Ledger(AtomicU64);
        impl MemoryLeaseOwner for Ledger {
            fn release(&self, bytes: u64) {
                let _previous = self.0.fetch_sub(bytes, Ordering::SeqCst);
            }
        }
        struct Scope(Arc<Ledger>);
        impl MemoryScopePort for Scope {
            fn reserve(
                &self,
                bytes: u64,
                _lifetime: MemoryLifetime,
            ) -> Result<MemoryLease, IndexError> {
                let _previous = self.0.0.fetch_add(bytes, Ordering::SeqCst);
                Ok(MemoryLease::new(bytes, self.0.clone()))
            }
        }
        let home = tempfile::tempdir()?;
        let store = Arc::new(FileObjectStore::open(home.path().join("store"))?);
        let (old, base, inputs, expected) = fixture(&store)?;
        let edit = MapEdit::objects(b"owner".to_vec(), Some(old), expected.get(1).copied());
        let ledger = Arc::new(Ledger(AtomicU64::new(0)));
        let mut work = budget();
        work.bind_memory_scope(Some(MemoryScope::new(Arc::new(Scope(ledger.clone())))));
        let (references, roots) = store.install_objects_and_maps(
            &inputs,
            &[base],
            &[&edit],
            &Sink::default(),
            &mut work,
        )?;
        let expected_live = u64::try_from(
            2_usize
                .checked_mul(std::mem::size_of::<ObjectRef>())
                .ok_or(IndexError::ResourceLimited)?,
        )?
        .checked_add(StagedMapRoots::allocation_bytes(1)?)
        .ok_or(IndexError::ResourceLimited)?;
        require(ledger.0.load(Ordering::SeqCst) == expected_live)?;
        let shared = roots.clone();
        drop(references);
        drop(roots);
        require(ledger.0.load(Ordering::SeqCst) == StagedMapRoots::allocation_bytes(1)?)?;
        drop(shared);
        require(ledger.0.load(Ordering::SeqCst) == 0)?;
        let token = Arc::clone(&store).prepare_one_map(base, &edit, attempt(), &mut work)?;
        require(ledger.0.load(Ordering::SeqCst) > 0)?;
        drop(token);
        require(ledger.0.load(Ordering::SeqCst) == 0)?;
        Ok(())
    }
}
