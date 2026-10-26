use std::{
    collections::{BTreeMap, BTreeSet},
    sync::{
        Mutex,
        atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering},
    },
    time::{Duration, Instant},
};

use quanta_index_core::index::{
    AttemptContext, AttemptId, AttemptRef, CollectionId, Digest, GroupActivation, IndexError,
    IndexInstanceId, IndexKind, InputDomain, InstanceSpec, MapBatchRequest, MapBatchWrite, MapEdit,
    MapPage, MapRoot, MapValue, MapWrite, ObjectChildren, ObjectRef, OperationKey, OwnerCoverage,
    OwnerInputBinding, OwnerKey, ProducerId, ProjectionId, ProjectionOwnerContribution,
    PublicationGroupId, RegisteredObject, SchemaId, SlotId, StableOwnerId, StorageFormatId,
    StoredObject, StreamId, TenantId, WorkBudget, WorkLimits, WorkspaceKey, WorktreeId,
    owner_map_key,
    ports::{
        ImmutableObjectPort, MapDiffRange, MapDiffVisitor, MapEditCursor, ObjectCustodyPort,
        ObjectWriteSink, PersistentMapPort,
    },
};

use super::super::AnnDirectory;
use super::{key, refresh};

macro_rules! check {
    ($condition:expr $(,)?) => {
        if !$condition {
            return Err(IndexError::BackendViolation(format!(
                "reverse test expectation failed: {}",
                stringify!($condition)
            )));
        }
    };
}

macro_rules! check_eq {
    ($left:expr, $right:expr $(,)?) => {
        if $left != $right {
            return Err(IndexError::BackendViolation(format!(
                "reverse test expectation failed: {} == {}",
                stringify!($left),
                stringify!($right)
            )));
        }
    };
}

#[derive(Default)]
struct Objects {
    values: Mutex<BTreeMap<ObjectRef, StoredObject>>,
    next: AtomicU64,
}

impl Objects {
    fn insert(&self, payload: Vec<u8>, children: Vec<ObjectRef>) -> Result<ObjectRef, IndexError> {
        let serial = self
            .next
            .fetch_add(1, Ordering::SeqCst)
            .checked_add(1)
            .ok_or(IndexError::ResourceLimited)?;
        let mut digest = [0; 32];
        digest
            .get_mut(..8)
            .ok_or(IndexError::ResourceLimited)?
            .copy_from_slice(&serial.to_be_bytes());
        let reference = ObjectRef(Digest(digest));
        let object = StoredObject::new(
            RegisteredObject::unscoped(
                reference,
                u64::try_from(payload.len()).map_err(|_error| IndexError::ResourceLimited)?,
                children,
            ),
            payload,
            None,
        );
        let _previous = self
            .values
            .lock()
            .map_err(|_error| IndexError::Storage("reverse test object lock poisoned".into()))?
            .insert(reference, object);
        Ok(reference)
    }
}

impl ImmutableObjectPort for Objects {
    fn identify(
        &self,
        _payload: &[u8],
        _children: &[ObjectRef],
        _budget: &mut WorkBudget,
    ) -> Result<RegisteredObject, IndexError> {
        Err(IndexError::BackendViolation(
            "unexpected object identify".into(),
        ))
    }

    fn put(
        &self,
        _payload: &[u8],
        _children: &[ObjectRef],
        _budget: &mut WorkBudget,
    ) -> Result<RegisteredObject, IndexError> {
        Err(IndexError::BackendViolation("unexpected object put".into()))
    }

    fn install(
        &self,
        _payload: &[u8],
        _children: &[ObjectRef],
        _sink: &dyn ObjectWriteSink,
        _budget: &mut WorkBudget,
    ) -> Result<RegisteredObject, IndexError> {
        Err(IndexError::BackendViolation(
            "unexpected object install".into(),
        ))
    }

    fn install_batch(
        &self,
        _inputs: &[quanta_index_core::index::ObjectBatchInput],
        _sink: &dyn ObjectWriteSink,
        _budget: &mut WorkBudget,
    ) -> Result<quanta_index_core::index::RegisteredObjectBatch, IndexError> {
        Err(IndexError::BackendViolation(
            "unexpected object batch".into(),
        ))
    }

    fn get(
        &self,
        reference: ObjectRef,
        budget: &mut WorkBudget,
    ) -> Result<StoredObject, IndexError> {
        budget.checkpoint()?;
        self.values
            .lock()
            .map_err(|_error| IndexError::Storage("reverse test object lock poisoned".into()))?
            .get(&reference)
            .cloned()
            .ok_or_else(|| IndexError::CorruptObject("missing test object".into()))
    }
}

#[derive(Default)]
struct Maps {
    states: Mutex<Vec<MapSnapshot>>,
    next: AtomicU64,
    visits: AtomicUsize,
    applies: AtomicUsize,
    fail_diff: AtomicBool,
    fail_apply: AtomicBool,
    fail_on_apply: AtomicUsize,
    repeat_cursor: AtomicBool,
}

type MapSnapshot = (MapRoot, BTreeMap<Vec<u8>, MapValue>);

impl Maps {
    fn seed(&self, state: BTreeMap<Vec<u8>, MapValue>) -> Result<MapRoot, IndexError> {
        if state.is_empty() {
            return Ok(MapRoot(None));
        }
        let serial = self
            .next
            .fetch_add(1, Ordering::SeqCst)
            .checked_add(1)
            .ok_or(IndexError::ResourceLimited)?;
        let mut digest = [0; 32];
        digest
            .get_mut(..8)
            .ok_or(IndexError::ResourceLimited)?
            .copy_from_slice(&serial.to_be_bytes());
        *digest.get_mut(8).ok_or(IndexError::ResourceLimited)? = 0x7f;
        let root = MapRoot(Some(ObjectRef(Digest(digest))));
        self.states
            .lock()
            .map_err(|_error| IndexError::Storage("reverse test map lock poisoned".into()))?
            .push((root, state));
        Ok(root)
    }

    fn state(&self, root: MapRoot) -> Result<BTreeMap<Vec<u8>, MapValue>, IndexError> {
        if root.0.is_none() {
            return Ok(BTreeMap::new());
        }
        self.states
            .lock()
            .map_err(|_error| IndexError::Storage("reverse test map lock poisoned".into()))?
            .iter()
            .find(|(candidate, _)| *candidate == root)
            .map(|(_, state)| state.clone())
            .ok_or_else(|| IndexError::CorruptObject("missing test map root".into()))
    }
}

impl PersistentMapPort for Maps {
    fn lookup(
        &self,
        root: MapRoot,
        key: &[u8],
        budget: &mut WorkBudget,
    ) -> Result<Option<MapValue>, IndexError> {
        budget.checkpoint()?;
        Ok(self.state(root)?.get(key).copied())
    }

    fn scan(
        &self,
        root: MapRoot,
        after: Option<&[u8]>,
        limit: u32,
        budget: &mut WorkBudget,
    ) -> Result<MapPage, IndexError> {
        budget.checkpoint()?;
        let mut rows: Vec<_> = self
            .state(root)?
            .into_iter()
            .filter(|(key, _)| after.is_none_or(|cursor| key.as_slice() > cursor))
            .collect();
        let limit = usize::try_from(limit).map_err(|_error| IndexError::ResourceLimited)?;
        let has_more = rows.len() > limit;
        rows.truncate(limit);
        Ok(MapPage::new(rows, has_more, Vec::new(), None))
    }

    fn visit_diff(
        &self,
        before: MapRoot,
        after: MapRoot,
        range: MapDiffRange<'_>,
        visitor: &mut dyn MapDiffVisitor,
        budget: &mut WorkBudget,
    ) -> Result<bool, IndexError> {
        let _previous = self.visits.fetch_add(1, Ordering::SeqCst);
        if self.fail_diff.swap(false, Ordering::SeqCst) {
            return Err(IndexError::Storage(
                "injected structural diff failure".into(),
            ));
        }
        if range.limit == 0 {
            return Err(IndexError::InvalidInput("zero test diff page".into()));
        }
        let old = self.state(before)?;
        let new = self.state(after)?;
        if let Some(cursor) = range
            .after
            .filter(|_cursor| self.repeat_cursor.load(Ordering::SeqCst))
        {
            let previous = old.get(cursor).copied();
            let next = new.get(cursor).copied();
            if previous == next {
                return Err(IndexError::BackendViolation(
                    "repeat-cursor fixture has no changed key".into(),
                ));
            }
            visitor.changed(cursor, previous, next, budget)?;
            return Ok(false);
        }
        // Test-only finite fixture. Production backends retain their bounded
        // structural-diff requirement and fail-closed default.
        let keys: BTreeSet<_> = old.keys().chain(new.keys()).cloned().collect();
        let mut seen = 0u32;
        for key in keys {
            if !key.starts_with(range.prefix)
                || range.after.is_some_and(|cursor| key.as_slice() <= cursor)
            {
                continue;
            }
            let previous = old.get(&key).copied();
            let next = new.get(&key).copied();
            if previous == next {
                continue;
            }
            if seen == range.limit {
                return Ok(true);
            }
            visitor.changed(&key, previous, next, budget)?;
            seen = seen.checked_add(1).ok_or(IndexError::ResourceLimited)?;
        }
        Ok(false)
    }

    fn apply(
        &self,
        base: MapRoot,
        edits: &[MapEdit],
        _sink: &dyn ObjectWriteSink,
        budget: &mut WorkBudget,
    ) -> Result<MapWrite, IndexError> {
        let ordinal = self
            .applies
            .fetch_add(1, Ordering::SeqCst)
            .checked_add(1)
            .ok_or(IndexError::ResourceLimited)?;
        if self.fail_apply.swap(false, Ordering::SeqCst)
            || self.fail_on_apply.load(Ordering::SeqCst) == ordinal
        {
            return Err(IndexError::Storage("injected reverse apply failure".into()));
        }
        budget.checkpoint()?;
        if edits
            .windows(2)
            .any(|pair| matches!(pair, [left, right] if left.key >= right.key))
        {
            return Err(IndexError::InvalidInput("unsorted test map edits".into()));
        }
        let mut state = self.state(base)?;
        for edit in edits {
            if state.get(&edit.key).copied() != edit.expected {
                return Err(IndexError::Conflict);
            }
            if let Some(next) = edit.next {
                let _previous = state.insert(edit.key.clone(), next);
            } else {
                let _removed = state.remove(&edit.key);
            }
        }
        Ok(MapWrite {
            root: self.seed(state)?,
        })
    }

    fn apply_batch(
        &self,
        _requests: &[MapBatchRequest],
        _sink: &dyn ObjectWriteSink,
        _budget: &mut WorkBudget,
    ) -> Result<MapBatchWrite, IndexError> {
        Err(IndexError::BackendViolation("unexpected map batch".into()))
    }

    fn apply_stream(
        &self,
        _bases: &[MapRoot],
        _edits: &mut dyn MapEditCursor,
        _sink: &dyn ObjectWriteSink,
        _budget: &mut WorkBudget,
    ) -> Result<quanta_index_core::index::StagedMapRoots, IndexError> {
        Err(IndexError::BackendViolation("unexpected map stream".into()))
    }
}

struct Custody;
impl ObjectCustodyPort for Custody {
    fn reserve_objects(
        &self,
        _attempt: AttemptRef,
        _objects: &[RegisteredObject],
    ) -> Result<(), IndexError> {
        Ok(())
    }
    fn register_objects(
        &self,
        _attempt: AttemptRef,
        _objects: &[RegisteredObject],
    ) -> Result<(), IndexError> {
        Ok(())
    }
}

struct Scratch;
impl quanta_index_core::index::ScratchSpacePort for Scratch {
    fn allocate(
        &self,
        _request: quanta_index_core::index::ScratchRequest,
        _budget: &mut WorkBudget,
    ) -> Result<Box<dyn quanta_index_core::index::ScratchAllocation>, IndexError> {
        Err(IndexError::BackendViolation("unexpected scratch".into()))
    }
}

fn owner(producer: u8, id: u8) -> OwnerKey {
    OwnerKey {
        producer: ProducerId([producer; 16]),
        stable_id: StableOwnerId([id; 16]),
    }
}

fn projected_instance() -> InstanceSpec {
    InstanceSpec {
        instance: IndexInstanceId([11; 16]),
        kind: IndexKind([12; 16]),
        group: PublicationGroupId([13; 16]),
        activation: GroupActivation::AsyncCoverage,
        input_domain: InputDomain::Projection {
            projection: ProjectionId([14; 16]),
            producer: ProducerId([15; 16]),
        },
        schema: SchemaId([16; 16]),
        policy: Digest([17; 32]),
        format: StorageFormatId([18; 16]),
    }
}

fn attempt() -> AttemptRef {
    AttemptRef {
        operation: OperationKey {
            workspace: WorkspaceKey {
                tenant: TenantId([1; 16]),
                collection: CollectionId([1; 16]),
                worktree: WorktreeId([1; 16]),
                slot: SlotId([1; 16]),
            },
            epoch: 1,
            producer: ProducerId([15; 16]),
            stream: StreamId([1; 16]),
            stream_epoch: 1,
            sequence: 1,
        },
        attempt: AttemptId([1; 16]),
        fence: 1,
    }
}

fn budget() -> Result<WorkBudget, IndexError> {
    Ok(WorkBudget::new(
        WorkLimits {
            reads: 1000,
            writes: 1000,
            bytes: 1_000_000,
            mutations: 1000,
        },
        Instant::now()
            .checked_add(Duration::from_secs(5))
            .ok_or(IndexError::ResourceLimited)?,
    ))
}

fn coverage(
    objects: &Objects,
    instance: &InstanceSpec,
    semantic: OwnerKey,
    source_owner: Option<OwnerKey>,
    source: Option<ObjectRef>,
    revision: u8,
) -> Result<MapValue, IndexError> {
    let InputDomain::Projection {
        projection,
        producer,
    } = instance.input_domain
    else {
        return Err(IndexError::InvalidInput(
            "reverse test fixture requires projection instance".into(),
        ));
    };
    let contribution = ProjectionOwnerContribution {
        projection,
        producer,
        owner: semantic,
        source_owner,
        source,
        dependency_stamp: Digest([revision; 32]),
        output: None,
    };
    let projection_ref = objects.insert(
        serde_json::to_vec(&contribution)
            .map_err(|error| IndexError::Storage(format!("test contribution: {error}")))?,
        contribution.object_children(),
    )?;
    let coverage = OwnerCoverage {
        owner: semantic,
        desired: OwnerInputBinding {
            source,
            projection: Some(projection_ref),
            producer_context: contribution.materialization_context(),
            derivation_policy: instance.policy,
        },
        covered: None,
        searchable: None,
    };
    let children = ObjectChildren::from_references([source, Some(projection_ref), None, None]);
    Ok(MapValue::Object(
        objects.insert(
            serde_json::to_vec(&coverage)
                .map_err(|error| IndexError::Storage(format!("test coverage: {error}")))?,
            children.as_slice().to_vec(),
        )?,
    ))
}

fn owners(entries: &[(OwnerKey, MapValue)]) -> BTreeMap<Vec<u8>, MapValue> {
    entries
        .iter()
        .map(|(owner, value)| (owner_map_key(*owner).to_vec(), *value))
        .collect()
}

#[test]
fn refresh_tracks_shared_source_replacement_move_and_delete() -> Result<(), IndexError> {
    let objects = Objects::default();
    let maps = Maps::default();
    let custody = Custody;
    let scratch = Scratch;
    let instance = projected_instance();
    let original_source_owner = owner(1, 1);
    let destination_source_owner = owner(1, 2);
    let first_semantic_owner = owner(15, 1);
    let second_semantic_owner = owner(15, 2);
    let source_free_semantic_owner = owner(15, 3);
    let original_source_object = objects.insert(b"source A".to_vec(), Vec::new())?;
    let migrated_source_object = objects.insert(b"source B".to_vec(), Vec::new())?;
    let initial_first_coverage = coverage(
        &objects,
        &instance,
        first_semantic_owner,
        Some(original_source_owner),
        Some(original_source_object),
        1,
    )?;
    let initial_second_coverage = coverage(
        &objects,
        &instance,
        second_semantic_owner,
        Some(original_source_owner),
        Some(original_source_object),
        1,
    )?;
    let source_free = coverage(
        &objects,
        &instance,
        source_free_semantic_owner,
        None,
        None,
        1,
    )?;
    let first_root = maps.seed(owners(&[
        (first_semantic_owner, initial_first_coverage),
        (second_semantic_owner, initial_second_coverage),
        (source_free_semantic_owner, source_free),
    ]))?;
    let mut budget = budget()?;
    let mut context = AttemptContext {
        attempt: attempt(),
        objects: &objects,
        maps: &maps,
        custody: &custody,
        scratch: &scratch,
        budget: &mut budget,
    };
    let reverse = refresh(
        MapRoot(None),
        MapRoot(None),
        first_root,
        &instance,
        &mut context,
    )?;
    let state = maps.state(reverse)?;
    check_eq!(state.len(), 2);
    check_eq!(
        state.get(key(original_source_owner, first_semantic_owner).as_slice()),
        Some(&initial_first_coverage)
    );
    check_eq!(
        state.get(key(original_source_owner, second_semantic_owner).as_slice()),
        Some(&initial_second_coverage)
    );
    check!(!state.contains_key(key(original_source_owner, source_free_semantic_owner).as_slice()));

    let replacement = coverage(
        &objects,
        &instance,
        first_semantic_owner,
        Some(original_source_owner),
        Some(original_source_object),
        2,
    )?;
    let second_root = maps.seed(owners(&[
        (first_semantic_owner, replacement),
        (second_semantic_owner, initial_second_coverage),
        (source_free_semantic_owner, source_free),
    ]))?;
    let reverse = refresh(reverse, first_root, second_root, &instance, &mut context)?;
    let state = maps.state(reverse)?;
    check_eq!(
        state.get(key(original_source_owner, first_semantic_owner).as_slice()),
        Some(&replacement)
    );
    check_eq!(
        state.get(key(original_source_owner, second_semantic_owner).as_slice()),
        Some(&initial_second_coverage)
    );

    let migrated_coverage = coverage(
        &objects,
        &instance,
        second_semantic_owner,
        Some(destination_source_owner),
        Some(migrated_source_object),
        3,
    )?;
    let third_root = maps.seed(owners(&[
        (second_semantic_owner, migrated_coverage),
        (source_free_semantic_owner, source_free),
    ]))?;
    let reverse = refresh(reverse, second_root, third_root, &instance, &mut context)?;
    let state = maps.state(reverse)?;
    check_eq!(state.len(), 1);
    check!(!state.contains_key(key(original_source_owner, first_semantic_owner).as_slice()));
    check!(!state.contains_key(key(original_source_owner, second_semantic_owner).as_slice()));
    check_eq!(
        state.get(key(destination_source_owner, second_semantic_owner).as_slice()),
        Some(&migrated_coverage)
    );
    Ok(())
}

#[test]
fn failures_do_not_assign_directory_reverse_and_unchanged_root_does_no_work()
-> Result<(), IndexError> {
    let objects = Objects::default();
    let maps = Maps::default();
    let custody = Custody;
    let scratch = Scratch;
    let instance = projected_instance();
    let semantic = owner(15, 1);
    let source_owner = owner(1, 1);
    let source_ref = objects.insert(b"source".to_vec(), Vec::new())?;
    let value = coverage(
        &objects,
        &instance,
        semantic,
        Some(source_owner),
        Some(source_ref),
        1,
    )?;
    let target = maps.seed(owners(&[(semantic, value)]))?;
    let mut directory = AnnDirectory::empty(Digest([30; 32]), MapRoot(None));
    let mut budget = budget()?;
    let mut context = AttemptContext {
        attempt: attempt(),
        objects: &objects,
        maps: &maps,
        custody: &custody,
        scratch: &scratch,
        budget: &mut budget,
    };
    maps.fail_diff.store(true, Ordering::SeqCst);
    check!(matches!(
        directory.refresh_source_reverse(&instance, target, &mut context),
        Err(IndexError::Storage(_))
    ));
    check_eq!(directory.source_reverse, MapRoot(None));
    maps.fail_apply.store(true, Ordering::SeqCst);
    check!(matches!(
        directory.refresh_source_reverse(&instance, target, &mut context),
        Err(IndexError::Storage(_))
    ));
    check_eq!(directory.source_reverse, MapRoot(None));
    directory.refresh_source_reverse(&instance, target, &mut context)?;
    let reverse = directory.source_reverse;
    check_eq!(
        maps.state(reverse)?
            .get(key(source_owner, semantic).as_slice()),
        Some(&value)
    );
    directory.owners = target;
    let visits = maps.visits.load(Ordering::SeqCst);
    let applies = maps.applies.load(Ordering::SeqCst);
    directory.refresh_source_reverse(&instance, target, &mut context)?;
    check_eq!(directory.source_reverse, reverse);
    check_eq!(maps.visits.load(Ordering::SeqCst), visits);
    check_eq!(maps.applies.load(Ordering::SeqCst), applies);
    Ok(())
}

#[test]
fn second_page_failure_never_assigns_a_successful_prefix() -> Result<(), IndexError> {
    let objects = Objects::default();
    let maps = Maps::default();
    let custody = Custody;
    let scratch = Scratch;
    let instance = projected_instance();
    let source_owner = owner(1, 1);
    let source_ref = objects.insert(b"source".to_vec(), Vec::new())?;
    let mut entries = Vec::new();
    for id in 0..65u8 {
        let semantic = owner(15, id);
        entries.push((
            semantic,
            coverage(
                &objects,
                &instance,
                semantic,
                Some(source_owner),
                Some(source_ref),
                id,
            )?,
        ));
    }
    let target = maps.seed(owners(&entries))?;
    let mut directory = AnnDirectory::empty(Digest([30; 32]), MapRoot(None));
    let mut budget = budget()?;
    let mut context = AttemptContext {
        attempt: attempt(),
        objects: &objects,
        maps: &maps,
        custody: &custody,
        scratch: &scratch,
        budget: &mut budget,
    };
    maps.fail_on_apply.store(2, Ordering::SeqCst);
    check!(matches!(
        directory.refresh_source_reverse(&instance, target, &mut context),
        Err(IndexError::Storage(_))
    ));
    check_eq!(maps.applies.load(Ordering::SeqCst), 2);
    check_eq!(directory.source_reverse, MapRoot(None));
    maps.fail_on_apply.store(0, Ordering::SeqCst);
    maps.repeat_cursor.store(true, Ordering::SeqCst);
    check!(matches!(
        directory.refresh_source_reverse(&instance, target, &mut context),
        Err(IndexError::CorruptObject(_))
    ));
    check_eq!(maps.applies.load(Ordering::SeqCst), 3);
    check_eq!(directory.source_reverse, MapRoot(None));
    Ok(())
}
