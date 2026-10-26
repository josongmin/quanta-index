//! Actual `NativeSemantic` -> managed file graph -> selected ANN query roundtrip.
//!
//! The map/object fixtures only stand in for persistent ports; product
//! materialization, descriptor validation and Lance query stay in the path.

#[path = "ann_product_tests/consolidation.rs"]
mod consolidation;
#[path = "ann_product_tests/maintenance.rs"]
mod maintenance;

use std::{
    collections::{BTreeMap, BTreeSet},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicU64, Ordering as AtomicOrdering},
    },
    time::Instant,
};

use quanta_index_core::index::{
    MapEdit, MapPage, MapWrite, MemoryLease, MemoryLeaseOwner, MemoryLifetime, MemoryScope,
    MemoryScopePort, ObjectRef, OwnerCoverage, WorkLimits, WorkUsage, owner_from_map_key,
    owner_map_key,
    ports::{MapEditCursor, ObjectWriteSink},
};

use super::*;
use quanta_index_contract::native_protocol::NativeSemanticTraversal;

/// Immutable test roots are ordinary objects whose child edges include every
/// value. This makes reachability assertions independent of the map fixture's
/// in-memory snapshots.
type GraphSnapshot = (MapRoot, BTreeMap<Vec<u8>, ObjectRef>);

struct GraphMap<'a> {
    objects: &'a MemoryStore,
    states: Mutex<Vec<GraphSnapshot>>,
    scan_override: Mutex<Option<(MapRoot, MapPage)>>,
    lookup_calls: AtomicU64,
    scan_calls: AtomicU64,
}

impl<'a> GraphMap<'a> {
    fn new(objects: &'a MemoryStore) -> Self {
        Self {
            objects,
            states: Mutex::new(Vec::new()),
            scan_override: Mutex::new(None),
            lookup_calls: AtomicU64::new(0),
            scan_calls: AtomicU64::new(0),
        }
    }

    fn state(&self, root: MapRoot) -> Result<BTreeMap<Vec<u8>, ObjectRef>, IndexError> {
        if root.0.is_none() {
            return Ok(BTreeMap::new());
        }
        self.states
            .lock()
            .map_err(|_cause| IndexError::Storage("ANN test map lock poisoned".into()))?
            .iter()
            .find(|(held, _)| *held == root)
            .map(|(_, entries)| entries.clone())
            .ok_or_else(|| IndexError::CorruptObject("ANN test map root missing".into()))
    }

    fn seed(&self, entries: BTreeMap<Vec<u8>, ObjectRef>) -> Result<MapRoot, IndexError> {
        if entries.is_empty() {
            return Ok(MapRoot(None));
        }
        let wire: Vec<_> = entries.iter().collect();
        let payload = serde_json::to_vec(&wire)
            .map_err(|error| IndexError::Storage(format!("ANN test map encode: {error}")))?;
        let mut children: Vec<_> = entries.values().copied().collect();
        children.sort_unstable();
        children.dedup();
        let root = MapRoot(Some(self.objects.insert(payload, children)?));
        self.states
            .lock()
            .map_err(|_cause| IndexError::Storage("ANN test map lock poisoned".into()))?
            .push((root, entries));
        Ok(root)
    }
}

impl PersistentMapPort for GraphMap<'_> {
    fn lookup_object(
        &self,
        root: MapRoot,
        key: &[u8],
        budget: &mut WorkBudget,
    ) -> Result<Option<ObjectRef>, IndexError> {
        let _previous = self.lookup_calls.fetch_add(1, AtomicOrdering::SeqCst);
        budget.charge(WorkUsage {
            reads: 1,
            ..WorkUsage::default()
        })?;
        Ok(self.state(root)?.get(key).copied())
    }
    fn lookup(
        &self,
        root: MapRoot,
        key: &[u8],
        budget: &mut WorkBudget,
    ) -> Result<Option<quanta_index_core::index::MapValue>, IndexError> {
        self.lookup_object(root, key, budget)
            .map(|value| value.map(Into::into))
    }

    fn scan(
        &self,
        root: MapRoot,
        after: Option<&[u8]>,
        limit: u32,
        budget: &mut WorkBudget,
    ) -> Result<MapPage, IndexError> {
        let _previous = self.scan_calls.fetch_add(1, AtomicOrdering::SeqCst);
        budget.charge(WorkUsage {
            reads: 1,
            ..WorkUsage::default()
        })?;
        if limit == 0 {
            return Err(IndexError::InvalidInput("zero ANN test map page".into()));
        }
        let override_page = self
            .scan_override
            .lock()
            .map_err(|_cause| IndexError::Storage("ANN test scan override lock poisoned".into()))?;
        if let Some((selected_root, page)) = override_page.as_ref()
            && *selected_root == root
        {
            return Ok(page.clone());
        }
        drop(override_page);
        let entries = self.state(root)?;
        let mut selected: Vec<_> = entries
            .into_iter()
            .filter(|(key, _)| after.is_none_or(|cursor| key.as_slice() > cursor))
            .collect();
        let cap = usize::try_from(limit).map_err(|_cause| IndexError::ResourceLimited)?;
        let has_more = selected.len() > cap;
        selected.truncate(cap);
        budget.charge(WorkUsage {
            bytes: selected.iter().try_fold(0u64, |total, (key, _)| {
                total
                    .checked_add(
                        u64::try_from(key.len()).map_err(|_cause| IndexError::ResourceLimited)?,
                    )
                    .ok_or(IndexError::ResourceLimited)
            })?,
            ..WorkUsage::default()
        })?;
        Ok(MapPage::new(
            selected
                .into_iter()
                .map(|(key, value)| (key, value.into()))
                .collect(),
            has_more,
            Vec::new(),
            None,
        ))
    }

    fn apply(
        &self,
        base: MapRoot,
        edits: &[MapEdit],
        sink: &dyn ObjectWriteSink,
        budget: &mut WorkBudget,
    ) -> Result<MapWrite, IndexError> {
        budget.charge(WorkUsage {
            writes: 1,
            mutations: 1,
            ..WorkUsage::default()
        })?;
        if edits
            .windows(2)
            .any(|pair| matches!(pair, [left, right] if left.key >= right.key))
        {
            return Err(IndexError::InvalidInput(
                "ANN test map edits unsorted".into(),
            ));
        }
        let mut entries = self.state(base)?;
        for edit in edits {
            if entries.get(&edit.key).copied().map(Into::into) != edit.expected {
                return Err(IndexError::Conflict);
            }
            if let Some(value) = edit.next {
                let _previous = entries.insert(edit.key.clone(), value.object_ref()?);
            } else {
                let _removed = entries.remove(&edit.key);
            }
        }
        let root = self.seed(entries)?;
        if let Some(reference) = root.0 {
            let registered = self.objects.get(reference, budget)?.registration.clone();
            sink.reserve(&registered)?;
            sink.installed(&registered)?;
        }
        Ok(MapWrite { root })
    }

    fn apply_batch(
        &self,
        _: &[MapBatchRequest],
        _: &dyn ObjectWriteSink,
        _: &mut WorkBudget,
    ) -> Result<MapBatchWrite, IndexError> {
        Err(IndexError::BackendViolation(
            "unexpected ANN test map batch".into(),
        ))
    }

    fn apply_stream(
        &self,
        _: &[MapRoot],
        _: &mut dyn MapEditCursor,
        _: &dyn ObjectWriteSink,
        _: &mut WorkBudget,
    ) -> Result<quanta_index_core::index::StagedMapRoots, IndexError> {
        Err(IndexError::BackendViolation(
            "unexpected ANN test map stream".into(),
        ))
    }
}

struct SourceVectorProvider {
    recipe: EmbeddingRecipe,
    empty: bool,
}

impl EmbeddingProvider for SourceVectorProvider {
    fn model_identity(&self) -> Digest {
        self.recipe.model_identity
    }
    fn dimension(&self) -> u32 {
        self.recipe.dimension
    }
    fn normalization(&self) -> VectorNormalization {
        self.recipe.normalization
    }
    fn output_policy(&self) -> Digest {
        self.recipe.output_policy
    }
    fn embed<'a>(&'a self, input: EmbeddingInput<'a>) -> IndexFuture<'a, EmbeddingOutput> {
        Box::pin(async move {
            if input.request_budget.is_cancelled() {
                return Err(IndexError::Cancelled);
            }
            if self.empty {
                return Ok(EmbeddingOutput::unscoped(Vec::new()));
            }
            let values = if input.source_bytes.first() == Some(&b'X') {
                vec![1.0, 0.0]
            } else {
                vec![0.0, 1.0]
            };
            Ok(EmbeddingOutput::unscoped(vec![EmbeddingVector {
                id: b"whole-file".to_vec(),
                values,
            }]))
        })
    }
}

fn ann_budget() -> Result<WorkBudget, Box<dyn std::error::Error>> {
    let deadline = Instant::now()
        .checked_add(Duration::from_secs(90))
        .ok_or("ANN test deadline overflow")?;
    Ok(WorkBudget::new(
        WorkLimits {
            reads: 100_000,
            writes: 100_000,
            bytes: 1_500_000_000,
            mutations: 100_000,
        },
        deadline,
    ))
}

// Search runs with a real, bounded memory authority so zero-row results do
// not mask a positive reservation that was previously bypassed by WorkBudget.
#[derive(Default)]
struct QueryMemory {
    live: AtomicU64,
    reserved: AtomicU64,
    released: AtomicU64,
    calls: AtomicU64,
    descriptor_allowances_reserved: AtomicU64,
    descriptor_allowances_released: AtomicU64,
    invalid_release: AtomicBool,
}

const REGION_DECODE_ALLOWANCE: u64 = 3 * 256 * 1024;

impl MemoryLeaseOwner for QueryMemory {
    fn release(&self, bytes: u64) {
        if self
            .live
            .fetch_update(AtomicOrdering::SeqCst, AtomicOrdering::SeqCst, |live| {
                live.checked_sub(bytes)
            })
            .is_err()
        {
            self.invalid_release.store(true, AtomicOrdering::SeqCst);
        }
        let _previous = self.released.fetch_add(bytes, AtomicOrdering::SeqCst);
        if bytes == REGION_DECODE_ALLOWANCE {
            let _previous = self
                .descriptor_allowances_released
                .fetch_add(1, AtomicOrdering::SeqCst);
        }
    }
}

struct QueryMemoryScope(Arc<QueryMemory>);

impl MemoryScopePort for QueryMemoryScope {
    fn reserve(&self, bytes: u64, _lifetime: MemoryLifetime) -> Result<MemoryLease, IndexError> {
        // The fixture does not grant unbounded memory merely because the test
        // store is in process. Every admitted lease must also be returned.
        const CAPACITY: u64 = 128 * 1024 * 1024;
        if bytes == 0 {
            return Err(IndexError::InvalidInput(
                "zero ANN test memory reservation".into(),
            ));
        }
        let _previous_live = self
            .0
            .live
            .fetch_update(AtomicOrdering::SeqCst, AtomicOrdering::SeqCst, |live| {
                live.checked_add(bytes).filter(|next| *next <= CAPACITY)
            })
            .map_err(|_live| IndexError::ResourceLimited)?;
        let _previous = self.0.calls.fetch_add(1, AtomicOrdering::SeqCst);
        let _previous = self.0.reserved.fetch_add(bytes, AtomicOrdering::SeqCst);
        if bytes == REGION_DECODE_ALLOWANCE {
            let _previous = self
                .0
                .descriptor_allowances_reserved
                .fetch_add(1, AtomicOrdering::SeqCst);
        }
        Ok(MemoryLease::new(bytes, self.0.clone()))
    }
}

fn check_query_memory_released(memory: &QueryMemory) -> Result<(), Box<dyn std::error::Error>> {
    check!(memory.calls.load(AtomicOrdering::SeqCst) > 0);
    check!(memory.reserved.load(AtomicOrdering::SeqCst) > 0);
    check_eq!(memory.live.load(AtomicOrdering::SeqCst), 0);
    check_eq!(
        memory.released.load(AtomicOrdering::SeqCst),
        memory.reserved.load(AtomicOrdering::SeqCst)
    );
    check!(!memory.invalid_release.load(AtomicOrdering::SeqCst));
    Ok(())
}

fn graph_reaches(
    store: &MemoryStore,
    root: ObjectRef,
    target: ObjectRef,
) -> Result<bool, IndexError> {
    let objects = store
        .0
        .lock()
        .map_err(|_cause| IndexError::Storage("ANN test object lock poisoned".into()))?;
    let mut seen = BTreeSet::new();
    let mut pending = vec![root];
    while let Some(reference) = pending.pop() {
        if !seen.insert(reference) {
            continue;
        }
        if reference == target {
            return Ok(true);
        }
        let object = objects
            .get(&reference)
            .ok_or_else(|| IndexError::CorruptObject("ANN test child graph broken".into()))?;
        pending.extend(object.registration.children.iter().copied());
    }
    drop(objects);
    Ok(false)
}

fn selected_search(
    runtime: &tokio::runtime::Runtime,
    store: &dyn ImmutableObjectPort,
    maps: &dyn PersistentMapPort,
    coverage: &CoverageBinding,
    source_owners: MapRoot,
    spec: &InstanceSpec,
    recipe: &EmbeddingRecipe,
) -> Result<(SearchResult, Arc<QueryMemory>), Box<dyn std::error::Error>> {
    let mut work = ann_budget()?;
    let memory = Arc::new(QueryMemory::default());
    work.bind_memory_scope(Some(MemoryScope::new(Arc::new(QueryMemoryScope(
        Arc::clone(&memory),
    )))));
    let mut read = ReadContext {
        objects: store,
        maps,
        budget: &mut work,
    };
    let result = crate::run_blocking(
        runtime,
        ann_query::search(
            &SearchQuery {
                lexical_eligibility: None,
                lexical_owners: None,
                exact_path: None,
                coverage,
                projection: None,
                source_owners,
                source_paths: MapRoot(None),
                instance: spec.instance,
                format: spec.format,
                recipe,
                vector: &[1.0, 0.0],
                top_k: 1,
                page_size: 64,
                max_owners: 80,
                overlay: None,
            },
            &mut read,
        ),
    )?;
    Ok((result, memory))
}

/// The selected descriptor may be physically opened once under this query.
/// Every other immutable read still delegates to the independent test store.
struct ReadOnceDescriptor<'a> {
    store: &'a MemoryStore,
    reference: ObjectRef,
    reads: AtomicU64,
    retained_reads: AtomicU64,
    memory: Option<&'a QueryMemory>,
    descriptor_allowances_at_retained_read: AtomicU64,
    refuse_retained: bool,
}

impl ImmutableObjectPort for ReadOnceDescriptor<'_> {
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
        self.store.put(payload, children, budget)
    }

    fn install(
        &self,
        payload: &[u8],
        children: &[ObjectRef],
        sink: &dyn ObjectWriteSink,
        budget: &mut WorkBudget,
    ) -> Result<RegisteredObject, IndexError> {
        self.store.install(payload, children, sink, budget)
    }

    fn install_batch(
        &self,
        inputs: &[ObjectBatchInput],
        sink: &dyn ObjectWriteSink,
        budget: &mut WorkBudget,
    ) -> Result<quanta_index_core::index::RegisteredObjectBatch, IndexError> {
        self.store.install_batch(inputs, sink, budget)
    }

    fn get(
        &self,
        reference: ObjectRef,
        budget: &mut WorkBudget,
    ) -> Result<StoredObject, IndexError> {
        if reference == self.reference {
            let previous = self.reads.fetch_add(1, AtomicOrdering::SeqCst);
            if previous != 0 {
                return Err(IndexError::BackendViolation(
                    "selected ANN descriptor was physically reopened".into(),
                ));
            }
        }
        self.store.get(reference, budget)
    }

    fn read_retained(
        &self,
        object: &StoredObject,
        budget: &mut WorkBudget,
    ) -> Result<StoredObject, IndexError> {
        if object.registration.reference != self.reference {
            return self.store.read_retained(object, budget);
        }
        // Match MemoryStore::get's logical read charge while refusing another
        // physical get of the selected descriptor.
        budget.charge(WorkUsage {
            reads: 1,
            ..WorkUsage::default()
        })?;
        let _previous = self.retained_reads.fetch_add(1, AtomicOrdering::SeqCst);
        if let Some(memory) = self.memory {
            let reserved = memory
                .descriptor_allowances_reserved
                .load(AtomicOrdering::SeqCst);
            let released = memory
                .descriptor_allowances_released
                .load(AtomicOrdering::SeqCst);
            self.descriptor_allowances_at_retained_read.store(
                reserved.checked_sub(released).unwrap_or(u64::MAX),
                AtomicOrdering::SeqCst,
            );
        }
        if self.refuse_retained {
            return Err(IndexError::ResourceLimited);
        }
        Ok(object.clone())
    }
}

#[test]
fn delta_page_authenticates_scanned_reference_without_second_map_read()
-> Result<(), Box<dyn std::error::Error>> {
    let store = MemoryStore::default();
    let maps = GraphMap::new(&store);
    let recipe = recipe();
    let spec = instance(&recipe);
    let selected_owner = owner(1);
    let artifact = store.insert(b"delta member artifact".to_vec(), Vec::new())?;
    let input = OwnerInputBinding {
        source: None,
        projection: None,
        producer_context: Digest([0; 32]),
        derivation_policy: recipe.fingerprint(),
    };
    let member = OwnerMemberBinding {
        instance: spec.instance,
        owner: selected_owner,
        input,
        format: spec.format,
        artifact,
        logical_commitment: Digest([3; 32]),
    };
    let member_ref = store.insert(serde_json::to_vec(&member)?, vec![artifact])?;
    let delta = maps.seed(BTreeMap::from([(
        owner_map_key(selected_owner).to_vec(),
        member_ref,
    )]))?;
    let mut directory = ann_directory::AnnDirectory::empty(recipe.fingerprint(), MapRoot(None));
    directory.delta = delta;
    directory.delta_owners = 1;
    directory.delta_rows = 1;
    let mut work = ann_budget()?;
    let page = maps.scan(delta, None, 64, &mut work)?;
    check_eq!(page.entries.len(), 1);
    let (key, value) = page.entries.first().ok_or("missing delta page entry")?;
    let scanned_owner = owner_from_map_key(key)?;
    let scanned_ref = *value.as_object_ref()?;
    let loaded = directory.delta_member_from_page(
        scanned_owner,
        scanned_ref,
        &mut ReadContext {
            objects: &store,
            maps: &maps,
            budget: &mut work,
        },
    )?;
    check_eq!(loaded, member);
    check_eq!(maps.scan_calls.load(AtomicOrdering::SeqCst), 1);
    check_eq!(maps.lookup_calls.load(AtomicOrdering::SeqCst), 0);
    check_eq!(
        directory.delta_member(
            scanned_owner,
            &mut ReadContext {
                objects: &store,
                maps: &maps,
                budget: &mut work,
            },
        )?,
        Some((member.clone(), member_ref))
    );
    check_eq!(maps.lookup_calls.load(AtomicOrdering::SeqCst), 1);

    // The page reference is an input to authentication, not an authority to
    // accept a member with a different owner or child graph.
    check!(matches!(
        directory.delta_member_from_page(
            owner(2),
            member_ref,
            &mut ReadContext {
                objects: &store,
                maps: &maps,
                budget: &mut work,
            },
        ),
        Err(IndexError::CorruptObject(_))
    ));
    let wrong_children = store.insert(serde_json::to_vec(&member)?, Vec::new())?;
    check!(matches!(
        directory.delta_member_from_page(
            scanned_owner,
            wrong_children,
            &mut ReadContext {
                objects: &store,
                maps: &maps,
                budget: &mut work,
            },
        ),
        Err(IndexError::CorruptObject(_))
    ));
    check_eq!(maps.lookup_calls.load(AtomicOrdering::SeqCst), 1);

    // A malformed scanned key still reaches the query's canonical owner-key
    // decoder before any member can be accepted.
    let bad_delta = maps.seed(BTreeMap::from([(b"bad-key".to_vec(), member_ref)]))?;
    directory.delta = bad_delta;
    let custody = TestCustody;
    let searchable = directory.save(&mut AttemptContext {
        attempt: attempt(),
        objects: &store,
        maps: &maps,
        custody: &custody,
        scratch: &NoScratch,
        budget: &mut work,
    })?;
    let desired = InputBinding {
        root: MapRoot(None),
        schema: spec.schema,
        policy: recipe.fingerprint(),
        context: Digest([0; 32]),
    };
    let selected = CoverageBinding {
        instance: spec.instance,
        desired,
        covered: Some(desired),
        searchable: Some(searchable),
        status: CoverageStatus::Complete,
        owners: MapRoot(None),
    };
    let runtime = test_runtime()?;
    let rejected = selected_search(
        &runtime,
        &store,
        &maps,
        &selected,
        MapRoot(None),
        &spec,
        &recipe,
    )
    .err()
    .ok_or("malformed delta key was accepted")?;
    check!(matches!(
        rejected.downcast_ref::<IndexError>(),
        Some(IndexError::CorruptObject(_))
    ));

    directory.delta = delta;
    let searchable = directory.save(&mut AttemptContext {
        attempt: attempt(),
        objects: &store,
        maps: &maps,
        custody: &custody,
        scratch: &NoScratch,
        budget: &mut work,
    })?;
    let selected = CoverageBinding {
        searchable: Some(searchable),
        ..selected
    };
    *maps
        .scan_override
        .lock()
        .map_err(|_cause| "ANN test scan override lock poisoned")? = Some((
        delta,
        MapPage::new(
            vec![(
                owner_map_key(selected_owner).to_vec(),
                MapValue::inline(b"not-an-object", [])?,
            )],
            false,
            Vec::new(),
            None,
        ),
    ));
    let rejected = selected_search(
        &runtime,
        &store,
        &maps,
        &selected,
        MapRoot(None),
        &spec,
        &recipe,
    )
    .err()
    .ok_or("non-object delta page value was accepted")?;
    check!(matches!(
        rejected.downcast_ref::<IndexError>(),
        Some(IndexError::CorruptObject(_))
    ));
    *maps
        .scan_override
        .lock()
        .map_err(|_cause| "ANN test scan override lock poisoned")? =
        Some((delta, MapPage::new(Vec::new(), true, Vec::new(), None)));
    let rejected = selected_search(
        &runtime,
        &store,
        &maps,
        &selected,
        MapRoot(None),
        &spec,
        &recipe,
    )
    .err()
    .ok_or("non-progressing delta page was accepted")?;
    check!(matches!(
        rejected.downcast_ref::<IndexError>(),
        Some(IndexError::BackendViolation(_))
    ));
    Ok(())
}

#[test]
#[expect(
    clippy::disallowed_methods,
    reason = "the regression polls publication without a Tokio executor to verify typed refusal before a backend panic"
)]
fn native_ann_selected_region_survives_replacement_and_deletion()
-> Result<(), Box<dyn std::error::Error>> {
    let runtime = test_runtime()?;
    let store = MemoryStore::default();
    let maps = GraphMap::new(&store);
    let custody = TestCustody;
    let recipe = recipe();
    let spec = instance(&recipe);
    let adapter = NativeSemantic::new(
        SourceVectorProvider {
            recipe: recipe.clone(),
            empty: false,
        },
        recipe.clone(),
    )?
    .with_ann();
    let mut source_entries = BTreeMap::new();
    let mut coverage_entries = BTreeMap::new();
    let mut prior = None;
    let mut old_coverage = None;
    let mut old_sources = MapRoot(None);

    for number in 1..=64u8 {
        let source_text = if number == 1 {
            b"X selected".as_slice()
        } else {
            b"Y candidate".as_slice()
        };
        let (source, _) = semantic_source(&store, &format!("src/{number}.rs"), source_text)?;
        let input = OwnerInputBinding {
            source: Some(source),
            projection: None,
            producer_context: Digest([0; 32]),
            derivation_policy: recipe.fingerprint(),
        };
        let mut work = ann_budget()?;
        let member = {
            let mut context = AttemptContext {
                attempt: attempt(),
                objects: &store,
                maps: &maps,
                custody: &custody,
                scratch: &NoScratch,
                budget: &mut work,
            };
            let candidate = crate::run_blocking(
                &runtime,
                adapter.prepare_owner(&spec, owner(number), input, None, &mut context),
            )?;
            crate::run_blocking(&runtime, adapter.verify_owner(candidate, &mut context))?
        };
        let member_children = ObjectChildren::from_references([
            Some(member.artifact),
            member.input.source,
            member.input.projection,
            None,
        ]);
        let member_ref = store.insert(
            serde_json::to_vec(&member)?,
            member_children.as_slice().to_vec(),
        )?;
        let owner_coverage = OwnerCoverage {
            owner: owner(number),
            desired: input,
            covered: Some(input),
            searchable: Some(member_ref),
        };
        let coverage_children = ObjectChildren::from_references([
            input.source,
            input.projection,
            Some(member_ref),
            None,
        ]);
        let coverage_ref = store.insert(
            serde_json::to_vec(&owner_coverage)?,
            coverage_children.as_slice().to_vec(),
        )?;
        check_eq!(
            source_entries.insert(owner_map_key(owner(number)).to_vec(), source),
            None
        );
        check_eq!(
            coverage_entries.insert(owner_map_key(owner(number)).to_vec(), coverage_ref),
            None
        );
        let source_owners = maps.seed(source_entries.clone())?;
        let owners = maps.seed(coverage_entries.clone())?;
        let searchable = {
            let mut context = AttemptContext {
                attempt: attempt(),
                objects: &store,
                maps: &maps,
                custody: &custody,
                scratch: &NoScratch,
                budget: &mut work,
            };
            if number == 64 {
                let refused = futures::executor::block_on(adapter.prepare_searchable(
                    &spec,
                    prior.as_ref(),
                    owners,
                    Some((owner(number), member_ref)),
                    None,
                    &mut context,
                ));
                check!(matches!(
                    refused,
                    Err(IndexError::BackendViolation(message))
                        if message == "Native ANN publication requires a Tokio runtime"
                ));
            }
            crate::run_blocking(
                &runtime,
                adapter.prepare_searchable(
                    &spec,
                    prior.as_ref(),
                    owners,
                    Some((owner(number), member_ref)),
                    None,
                    &mut context,
                ),
            )?
            .install(&mut context)?
            .ok_or("ANN searchable directory absent")?
        };
        let desired = quanta_index_core::index::InputBinding {
            root: source_owners,
            schema: spec.schema,
            policy: recipe.fingerprint(),
            context: Digest([0; 32]),
        };
        let coverage = CoverageBinding {
            instance: spec.instance,
            desired,
            covered: Some(desired),
            searchable: Some(searchable),
            status: CoverageStatus::Complete,
            owners,
        };
        if number == 64 {
            old_sources = source_owners;
            old_coverage = Some(coverage);
        }
        prior = Some(coverage);
    }

    let old = old_coverage.ok_or("ANN sealed coverage absent")?;
    let old_directory_ref = old.searchable.ok_or("ANN old directory absent")?;
    let mut read_work = ann_budget()?;
    let directory = ann_directory::AnnDirectory::load(
        old_directory_ref,
        recipe.fingerprint(),
        &mut ReadContext {
            objects: &store,
            maps: &maps,
            budget: &mut read_work,
        },
    )?;
    check_eq!(directory.next_region, 1);
    check_eq!(directory.delta_owners, 0);
    let region = directory.region_at(
        0,
        &mut ReadContext {
            objects: &store,
            maps: &maps,
            budget: &mut read_work,
        },
    )?;
    let fileset =
        crate::native::ann_files::load_fileset(&store, region.descriptor.fileset, &mut read_work)?;
    check_eq!(fileset.seal.rows, 64);
    check!(!fileset.files.is_empty());
    let first_chunk = fileset
        .files
        .iter()
        .flat_map(|file| &file.chunks)
        .next()
        .ok_or("ANN selected file chunk absent")?
        .object
        .registration
        .reference;
    check!(graph_reaches(&store, old_directory_ref, first_chunk)?);

    let measured = consolidation::RegionLookupMap::new(&maps);
    let selected_descriptor = ReadOnceDescriptor {
        store: &store,
        reference: region.reference(),
        reads: AtomicU64::new(0),
        retained_reads: AtomicU64::new(0),
        memory: None,
        descriptor_allowances_at_retained_read: AtomicU64::new(0),
        refuse_retained: false,
    };
    let (before, before_memory) = selected_search(
        &runtime,
        &selected_descriptor,
        &measured,
        &old,
        old_sources,
        &spec,
        &recipe,
    )?;
    check_eq!(before.hits.len(), 1);
    let before_hit = before.hits.first().ok_or("initial ANN hit absent")?;
    check_eq!(before_hit.owner, owner(1));
    check_eq!(before_hit.vector_id, b"whole-file".to_vec());
    check_eq!(before_hit.cosine.total_cmp(&1.0), Ordering::Equal);
    check_eq!(selected_descriptor.reads.load(AtomicOrdering::SeqCst), 1);
    check_eq!(
        selected_descriptor
            .retained_reads
            .load(AtomicOrdering::SeqCst),
        1
    );
    // Routing and the selected-region open retain the immutable page's
    // descriptor reference, so neither repeats a region-map point lookup.
    check_eq!(measured.region_lookups(), 0);
    drop(before);
    check_query_memory_released(&before_memory)?;

    let refused_memory = Arc::new(QueryMemory::default());
    let refusing_descriptor = ReadOnceDescriptor {
        store: &store,
        reference: region.reference(),
        reads: AtomicU64::new(0),
        retained_reads: AtomicU64::new(0),
        memory: Some(refused_memory.as_ref()),
        descriptor_allowances_at_retained_read: AtomicU64::new(u64::MAX),
        refuse_retained: true,
    };
    let mut refused_work = ann_budget()?;
    refused_work.bind_memory_scope(Some(MemoryScope::new(Arc::new(QueryMemoryScope(
        Arc::clone(&refused_memory),
    )))));
    let refused = crate::run_blocking(
        &runtime,
        ann_query::search(
            &SearchQuery {
                lexical_eligibility: None,
                lexical_owners: None,
                exact_path: None,
                coverage: &old,
                projection: None,
                source_owners: old_sources,
                source_paths: MapRoot(None),
                instance: spec.instance,
                format: spec.format,
                recipe: &recipe,
                vector: &[1.0, 0.0],
                top_k: 1,
                page_size: 64,
                max_owners: 80,
                overlay: None,
            },
            &mut ReadContext {
                objects: &refusing_descriptor,
                maps: &measured,
                budget: &mut refused_work,
            },
        ),
    );
    check!(matches!(refused, Err(IndexError::ResourceLimited)));
    check_eq!(refusing_descriptor.reads.load(AtomicOrdering::SeqCst), 1);
    check_eq!(
        refusing_descriptor
            .retained_reads
            .load(AtomicOrdering::SeqCst),
        1
    );
    check!(
        refused_memory
            .descriptor_allowances_reserved
            .load(AtomicOrdering::SeqCst)
            >= 1
    );
    // read_retained observes the routed DATA before the selected decode.
    // Routing must not keep the original 3x metadata decode allowance alive.
    check_eq!(
        refusing_descriptor
            .descriptor_allowances_at_retained_read
            .load(AtomicOrdering::SeqCst),
        0
    );
    check_query_memory_released(&refused_memory)?;

    // A replacement changes the selected owner map and adds a delta. The old
    // view keeps its immutable region and original source binding.
    let (replacement_source, _) = semantic_source(&store, "src/1.rs", b"Y replacement")?;
    let replacement_input = OwnerInputBinding {
        source: Some(replacement_source),
        projection: None,
        producer_context: Digest([0; 32]),
        derivation_policy: recipe.fingerprint(),
    };
    let mut work = ann_budget()?;
    let replacement_member = {
        let mut context = AttemptContext {
            attempt: attempt(),
            objects: &store,
            maps: &maps,
            custody: &custody,
            scratch: &NoScratch,
            budget: &mut work,
        };
        let candidate = crate::run_blocking(
            &runtime,
            adapter.prepare_owner(&spec, owner(1), replacement_input, None, &mut context),
        )?;
        crate::run_blocking(&runtime, adapter.verify_owner(candidate, &mut context))?
    };
    let member_children = ObjectChildren::from_references([
        Some(replacement_member.artifact),
        replacement_input.source,
        None,
        None,
    ]);
    let replacement_ref = store.insert(
        serde_json::to_vec(&replacement_member)?,
        member_children.as_slice().to_vec(),
    )?;
    let selected_owner = OwnerCoverage {
        owner: owner(1),
        desired: replacement_input,
        covered: Some(replacement_input),
        searchable: Some(replacement_ref),
    };
    let coverage_children = ObjectChildren::from_references([
        replacement_input.source,
        None,
        Some(replacement_ref),
        None,
    ]);
    let selected_ref = store.insert(
        serde_json::to_vec(&selected_owner)?,
        coverage_children.as_slice().to_vec(),
    )?;
    check!(
        source_entries
            .insert(owner_map_key(owner(1)).to_vec(), replacement_source)
            .is_some()
    );
    check!(
        coverage_entries
            .insert(owner_map_key(owner(1)).to_vec(), selected_ref)
            .is_some()
    );
    let replaced_sources = maps.seed(source_entries.clone())?;
    let replaced_owners = maps.seed(coverage_entries.clone())?;
    let replaced_directory = {
        let mut context = AttemptContext {
            attempt: attempt(),
            objects: &store,
            maps: &maps,
            custody: &custody,
            scratch: &NoScratch,
            budget: &mut work,
        };
        crate::run_blocking(
            &runtime,
            adapter.prepare_searchable(
                &spec,
                Some(&old),
                replaced_owners,
                Some((owner(1), replacement_ref)),
                None,
                &mut context,
            ),
        )?
        .install(&mut context)?
        .ok_or("ANN replacement directory absent")?
    };
    let desired = quanta_index_core::index::InputBinding {
        root: replaced_sources,
        schema: spec.schema,
        policy: recipe.fingerprint(),
        context: Digest([0; 32]),
    };
    let replaced = CoverageBinding {
        instance: spec.instance,
        desired,
        covered: Some(desired),
        searchable: Some(replaced_directory),
        status: CoverageStatus::Complete,
        owners: replaced_owners,
    };
    let (still_old, _still_old_memory) =
        selected_search(&runtime, &store, &maps, &old, old_sources, &spec, &recipe)?;
    let still_old_hit = still_old.hits.first().ok_or("old ANN hit absent")?;
    check_eq!(still_old_hit.owner, owner(1));
    check_eq!(still_old_hit.cosine.total_cmp(&1.0), Ordering::Equal);
    let (now, _now_memory) = selected_search(
        &runtime,
        &store,
        &maps,
        &replaced,
        replaced_sources,
        &spec,
        &recipe,
    )?;
    check_eq!(now.hits.len(), 1);
    let now_hit = now.hits.first().ok_or("replacement ANN hit absent")?;
    check_eq!(now_hit.cosine.total_cmp(&0.0), Ordering::Equal);
    check!(graph_reaches(&store, replaced_directory, first_chunk)?);

    // A later selected map can delete the owner without deleting old views.
    check!(
        source_entries
            .remove(owner_map_key(owner(1)).as_slice())
            .is_some()
    );
    check!(
        coverage_entries
            .remove(owner_map_key(owner(1)).as_slice())
            .is_some()
    );
    let deleted_sources = maps.seed(source_entries)?;
    let deleted_owners = maps.seed(coverage_entries)?;
    let deleted_directory = {
        let mut context = AttemptContext {
            attempt: attempt(),
            objects: &store,
            maps: &maps,
            custody: &custody,
            scratch: &NoScratch,
            budget: &mut work,
        };
        crate::run_blocking(
            &runtime,
            adapter.prepare_searchable(
                &spec,
                Some(&replaced),
                deleted_owners,
                None,
                None,
                &mut context,
            ),
        )?
        .install(&mut context)?
        .ok_or("ANN deletion directory absent")?
    };
    let desired = quanta_index_core::index::InputBinding {
        root: deleted_sources,
        schema: spec.schema,
        policy: recipe.fingerprint(),
        context: Digest([0; 32]),
    };
    let deleted = CoverageBinding {
        instance: spec.instance,
        desired,
        covered: Some(desired),
        searchable: Some(deleted_directory),
        status: CoverageStatus::Complete,
        owners: deleted_owners,
    };
    let (after_delete, _after_delete_memory) = selected_search(
        &runtime,
        &store,
        &maps,
        &deleted,
        deleted_sources,
        &spec,
        &recipe,
    )?;
    check_eq!(after_delete.hits.len(), 1);
    let after_delete_hit = after_delete
        .hits
        .first()
        .ok_or("post-delete ANN hit absent")?;
    check_ne!(after_delete_hit.owner, owner(1));
    check!(graph_reaches(&store, old_directory_ref, first_chunk)?);

    let empty_directory_ref = {
        let mut context = AttemptContext {
            attempt: attempt(),
            objects: &store,
            maps: &maps,
            custody: &custody,
            scratch: &NoScratch,
            budget: &mut work,
        };
        crate::run_blocking(
            &runtime,
            adapter.prepare_searchable(
                &spec,
                Some(&deleted),
                MapRoot(None),
                None,
                None,
                &mut context,
            ),
        )?
        .install(&mut context)?
        .ok_or("empty ANN directory absent")?
    };
    let empty_directory = ann_directory::AnnDirectory::load(
        empty_directory_ref,
        recipe.fingerprint(),
        &mut ReadContext {
            objects: &store,
            maps: &maps,
            budget: &mut work,
        },
    )?;
    check_eq!(empty_directory.owners, MapRoot(None));
    check_eq!(empty_directory.regions, MapRoot(None));
    check_eq!(empty_directory.delta, MapRoot(None));
    check_eq!(empty_directory.next_region, 1);
    check_eq!(empty_directory.region_count, 0);
    check!(!graph_reaches(&store, empty_directory_ref, first_chunk)?);
    check!(graph_reaches(&store, old_directory_ref, first_chunk)?);

    let empty_desired = quanta_index_core::index::InputBinding {
        root: MapRoot(None),
        schema: spec.schema,
        policy: recipe.fingerprint(),
        context: Digest([0; 32]),
    };
    let empty_coverage = CoverageBinding {
        instance: spec.instance,
        desired: empty_desired,
        covered: Some(empty_desired),
        searchable: Some(empty_directory_ref),
        status: CoverageStatus::Complete,
        owners: MapRoot(None),
    };
    let (empty_result, empty_memory) = selected_search(
        &runtime,
        &store,
        &maps,
        &empty_coverage,
        MapRoot(None),
        &spec,
        &recipe,
    )?;
    check!(empty_result.hits.is_empty());
    check_eq!(
        empty_result.traversal,
        NativeSemanticTraversal::ExactExhausted
    );
    check!(empty_result.exhaustive);
    drop(empty_result);
    check_query_memory_released(&empty_memory)?;
    Ok(())
}

#[test]
fn empty_source_vectors_across_65_owners_do_not_seal_a_region()
-> Result<(), Box<dyn std::error::Error>> {
    let runtime = test_runtime()?;
    let store = MemoryStore::default();
    let maps = GraphMap::new(&store);
    let custody = TestCustody;
    let recipe = recipe();
    let spec = instance(&recipe);
    let adapter = NativeSemantic::new(
        SourceVectorProvider {
            recipe: recipe.clone(),
            empty: true,
        },
        recipe.clone(),
    )?
    .with_ann();
    let mut source_entries = BTreeMap::new();
    let mut coverage_entries = BTreeMap::new();
    let mut prior = None;
    let mut work = ann_budget()?;

    for number in 1..=65u8 {
        let bytes = format!("owner {number}");
        let (source, _) = semantic_source(&store, &format!("src/{number}.rs"), bytes.as_bytes())?;
        let input = OwnerInputBinding {
            source: Some(source),
            projection: None,
            producer_context: Digest([0; 32]),
            derivation_policy: recipe.fingerprint(),
        };
        let member = {
            let mut context = AttemptContext {
                attempt: attempt(),
                objects: &store,
                maps: &maps,
                custody: &custody,
                scratch: &NoScratch,
                budget: &mut work,
            };
            let candidate = crate::run_blocking(
                &runtime,
                adapter.prepare_owner(&spec, owner(number), input, None, &mut context),
            )?;
            crate::run_blocking(&runtime, adapter.verify_owner(candidate, &mut context))?
        };
        let member_children =
            ObjectChildren::from_references([Some(member.artifact), input.source, None, None]);
        let member_ref = store.insert(
            serde_json::to_vec(&member)?,
            member_children.as_slice().to_vec(),
        )?;
        let coverage = OwnerCoverage {
            owner: owner(number),
            desired: input,
            covered: Some(input),
            searchable: Some(member_ref),
        };
        let children =
            ObjectChildren::from_references([input.source, None, Some(member_ref), None]);
        let coverage_ref =
            store.insert(serde_json::to_vec(&coverage)?, children.as_slice().to_vec())?;
        check_eq!(
            source_entries.insert(owner_map_key(owner(number)).to_vec(), source),
            None
        );
        check_eq!(
            coverage_entries.insert(owner_map_key(owner(number)).to_vec(), coverage_ref),
            None
        );
        let sources = maps.seed(source_entries.clone())?;
        let owners = maps.seed(coverage_entries.clone())?;
        let directory_ref = {
            let mut context = AttemptContext {
                attempt: attempt(),
                objects: &store,
                maps: &maps,
                custody: &custody,
                scratch: &NoScratch,
                budget: &mut work,
            };
            crate::run_blocking(
                &runtime,
                adapter.prepare_searchable(
                    &spec,
                    prior.as_ref(),
                    owners,
                    Some((owner(number), member_ref)),
                    None,
                    &mut context,
                ),
            )?
            .install(&mut context)?
            .ok_or("empty ANN directory absent")?
        };
        let desired = quanta_index_core::index::InputBinding {
            root: sources,
            schema: spec.schema,
            policy: recipe.fingerprint(),
            context: Digest([0; 32]),
        };
        prior = Some(CoverageBinding {
            instance: spec.instance,
            desired,
            covered: Some(desired),
            searchable: Some(directory_ref),
            status: CoverageStatus::Complete,
            owners,
        });
    }

    let selected = prior.ok_or("65-owner coverage absent")?;
    let directory = ann_directory::AnnDirectory::load(
        selected.searchable.ok_or("65-owner directory absent")?,
        recipe.fingerprint(),
        &mut ReadContext {
            objects: &store,
            maps: &maps,
            budget: &mut work,
        },
    )?;
    check_eq!(directory.next_region, 0);
    check_eq!(directory.delta_owners, 0);
    check_eq!(directory.delta_rows, 0);
    check_eq!(directory.regions, MapRoot(None));
    let (empty_result, memory) = selected_search(
        &runtime,
        &store,
        &maps,
        &selected,
        selected.desired.root,
        &spec,
        &recipe,
    )?;
    check!(empty_result.hits.is_empty());
    check_eq!(
        empty_result.traversal,
        NativeSemanticTraversal::Approximate { capped: false }
    );
    check!(!empty_result.exhaustive);
    drop(empty_result);
    check_query_memory_released(&memory)?;
    Ok(())
}

#[test]
fn overlay_filling_owner_limit_needs_no_durable_owner_allocation()
-> Result<(), Box<dyn std::error::Error>> {
    let runtime = test_runtime()?;
    let store = MemoryStore::default();
    let maps = GraphMap::new(&store);
    let custody = TestCustody;
    let mut work = ann_budget()?;
    let calls = Arc::new(AtomicUsize::new(0));
    let bridge = TextEmbeddingBridge::new(FixedTextProvider {
        calls: Arc::clone(&calls),
        deadline: work.request().deadline(),
        normalization: EmbeddingNormalization::L2Unit,
    })?;
    let recipe = bridge.recipe(1024)?;
    let spec = instance(&recipe);
    let searchable = ann_directory::AnnDirectory::empty(recipe.fingerprint(), MapRoot(None)).save(
        &mut AttemptContext {
            attempt: attempt(),
            objects: &store,
            maps: &maps,
            custody: &custody,
            scratch: &NoScratch,
            budget: &mut work,
        },
    )?;
    let desired = InputBinding {
        root: MapRoot(None),
        schema: spec.schema,
        policy: recipe.fingerprint(),
        context: Digest([0; 32]),
    };
    let coverage = CoverageBinding {
        instance: spec.instance,
        desired,
        covered: Some(desired),
        searchable: Some(searchable),
        status: CoverageStatus::Complete,
        owners: MapRoot(None),
    };
    let documents = [overlay::SemanticOverlayDocument {
        path: None,
        owner: owner(1),
        revision: 1,
        text: None,
    }];
    let memory = Arc::new(QueryMemory::default());
    work.bind_memory_scope(Some(MemoryScope::new(Arc::new(QueryMemoryScope(
        Arc::clone(&memory),
    )))));
    let result = crate::run_blocking(
        &runtime,
        ann_query::search(
            &SearchQuery {
                lexical_eligibility: None,
                lexical_owners: None,
                exact_path: None,
                coverage: &coverage,
                projection: None,
                source_owners: MapRoot(None),
                source_paths: MapRoot(None),
                instance: spec.instance,
                format: spec.format,
                recipe: &recipe,
                vector: &[1.0, 0.0],
                top_k: 1,
                page_size: 1,
                max_owners: 1,
                overlay: Some(overlay::OverlaySearch {
                    documents: &documents,
                    bridge: &bridge,
                }),
            },
            &mut ReadContext {
                objects: &store,
                maps: &maps,
                budget: &mut work,
            },
        ),
    )?;
    check!(result.hits.is_empty());
    check_eq!(result.scanned_owners, 1);
    check_eq!(result.traversal, NativeSemanticTraversal::ExactExhausted);
    check!(result.exhaustive);
    check_eq!(calls.load(AtomicOrdering::SeqCst), 0);
    drop(result);
    check_query_memory_released(&memory)?;
    Ok(())
}

#[test]
fn legacy_owner_map_searchable_requires_rebuild_before_io() -> Result<(), Box<dyn std::error::Error>>
{
    let runtime = test_runtime()?;
    let store = MemoryStore::default();
    let maps = GraphMap::new(&store);
    let custody = TestCustody;
    let recipe = recipe();
    let spec = instance(&recipe);
    let adapter = NativeSemantic::new(
        SourceVectorProvider {
            recipe: recipe.clone(),
            empty: false,
        },
        recipe.clone(),
    )?
    .with_ann();
    let owner_root = MapRoot(Some(ObjectRef(Digest([0x45; 32]))));
    let desired = quanta_index_core::index::InputBinding {
        root: owner_root,
        schema: spec.schema,
        policy: recipe.fingerprint(),
        context: Digest([0; 32]),
    };
    let legacy = CoverageBinding {
        instance: spec.instance,
        desired,
        covered: Some(desired),
        searchable: owner_root.0,
        status: CoverageStatus::Complete,
        owners: owner_root,
    };
    check!(matches!(
        adapter.validate_coverage(&legacy),
        Err(IndexError::RebuildRequired)
    ));
    let mut work = ann_budget()?;
    let mut context = AttemptContext {
        attempt: attempt(),
        objects: &store,
        maps: &maps,
        custody: &custody,
        scratch: &NoScratch,
        budget: &mut work,
    };
    check!(matches!(
        crate::run_blocking(
            &runtime,
            adapter.prepare_searchable(&spec, Some(&legacy), owner_root, None, None, &mut context,)
        ),
        Err(IndexError::RebuildRequired)
    ));
    Ok(())
}
