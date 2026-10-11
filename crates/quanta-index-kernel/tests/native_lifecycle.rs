//! Native end-to-end application tests: real catalog, COW storage and literal
//! adapter. These do not qualify daemon transport or large-corpus latency.
use futures::executor::block_on;
use quanta_index_catalog::SqliteCatalog;
use quanta_index_core::index::{
    AdmissionOutcome, CollectionId, Digest, GroupActivation, ImmutableOperation, IndexError,
    IndexInstanceId, IndexKind, IndexProfile, IngressRef, InstanceSpec, MemberBinding, ObjectRef,
    OperationKey, OperationStatus, OwnerKey, OwnerMutation, OwnerMutationSet, ProducerId,
    PublicationGroupId, Refusal, SchemaId, SlotId, SourceCut, SourceEvent, SourceInput,
    SourceRecord, SourceTransition, StableOwnerId, StorageFormatId, StreamId, SubmitOutcome,
    TenantId, UpdateKind, UpdateRequest, ViewRef, ViewSelection, WorkBudget, WorkLimits,
    WorkspaceKey, WorktreeId, owner_map_key,
    ports::{ImmutableObjectPort, InputStagingPort, OperationJournalPort, PersistentMapPort},
};
use quanta_index_kernel::{AdapterDescriptor, AdapterRegistry, Kernel, KernelServices};
use quanta_index_lexical::native::{LiteralQuery, LiteralReadContext, NativeLiteralIndex};
use quanta_index_repomap::native::{FileMapReadContext, FilePathQuery, NativeFileMap, query_paths};
use quanta_index_storage::FileObjectStore;
use std::{
    error::Error,
    sync::Arc,
    time::{Duration, Instant},
};

type TestResult<T = ()> = Result<T, Box<dyn Error>>;

fn budget() -> WorkBudget {
    WorkBudget::new(
        WorkLimits {
            reads: 1_000_000,
            writes: 1_000_000,
            bytes: 512 * 1024 * 1024,
            mutations: 1_000_000,
        },
        Instant::now() + Duration::from_secs(60),
    )
}

fn large_fixture_budget() -> WorkBudget {
    WorkBudget::new(
        WorkLimits {
            reads: 20_000_000,
            writes: 20_000_000,
            bytes: 2 * 1024 * 1024 * 1024,
            mutations: 20_000_000,
        },
        Instant::now() + Duration::from_secs(600),
    )
}

fn workspace() -> WorkspaceKey {
    WorkspaceKey {
        tenant: TenantId([1; 16]),
        collection: CollectionId([2; 16]),
        worktree: WorktreeId([3; 16]),
        slot: SlotId([4; 16]),
    }
}

fn operation(sequence: u64) -> OperationKey {
    OperationKey {
        workspace: workspace(),
        epoch: 1,
        producer: ProducerId([5; 16]),
        stream: StreamId([6; 16]),
        stream_epoch: 1,
        sequence,
    }
}

fn owner(id: u8) -> OwnerKey {
    OwnerKey {
        producer: ProducerId([5; 16]),
        stable_id: StableOwnerId([id; 16]),
    }
}

fn large_owner(id: u32) -> OwnerKey {
    OwnerKey {
        producer: ProducerId([5; 16]),
        stable_id: StableOwnerId(u128::from(id).to_be_bytes()),
    }
}

fn spec() -> InstanceSpec {
    InstanceSpec {
        instance: IndexInstanceId([7; 16]),
        kind: IndexKind([8; 16]),
        group: PublicationGroupId([9; 16]),
        activation: GroupActivation::CoreRequired,
        input_domain: quanta_index_core::index::InputDomain::SourceLocal,
        schema: SchemaId([10; 16]),
        policy: Digest([11; 32]),
        format: StorageFormatId([12; 16]),
    }
}

fn file_spec() -> InstanceSpec {
    InstanceSpec {
        instance: IndexInstanceId([17; 16]),
        kind: IndexKind([18; 16]),
        format: StorageFormatId([19; 16]),
        ..spec()
    }
}

fn kernel(
    root: &std::path::Path,
) -> TestResult<(
    Kernel<SqliteCatalog>,
    Arc<FileObjectStore>,
    Arc<SqliteCatalog>,
)> {
    let objects = Arc::new(FileObjectStore::open(root.join("native-objects"))?);
    let catalog = Arc::new(SqliteCatalog::open(root, Duration::from_millis(100))?);
    let mut registry = AdapterRegistry::new();
    registry.register(
        AdapterDescriptor {
            kind: spec().kind,
            schema: spec().schema,
            format: spec().format,
        },
        NativeLiteralIndex,
        |delta| Ok(delta.clone()),
    )?;
    registry.register(
        AdapterDescriptor {
            kind: file_spec().kind,
            schema: file_spec().schema,
            format: file_spec().format,
        },
        NativeFileMap,
        |delta| Ok(delta.clone()),
    )?;
    let services = KernelServices::new(objects.clone(), objects.clone(), catalog.clone());
    Ok((Kernel::new(services, registry), objects, catalog))
}

fn published(outcome: SubmitOutcome) -> TestResult<ViewRef> {
    match outcome {
        SubmitOutcome::Terminal(OperationStatus::Published(view)) => Ok(view),
        other => Err(format!("expected published, got {other:?}").into()),
    }
}

fn configured(kernel: &Kernel<SqliteCatalog>, _objects: &FileObjectStore) -> TestResult<ViewRef> {
    let genesis = kernel.create_workspace(workspace(), 1, &mut budget())?;
    let ingress = kernel.begin_input(operation(1))?;
    let profile = kernel.stage_profile(
        ingress,
        &IndexProfile {
            instances: vec![spec(), file_spec()],
        },
        &mut budget(),
    )?;
    published(block_on(kernel.submit(
        UpdateRequest {
            operation: operation(1),
            expected_view: genesis,
            kind: UpdateKind::Reconfigure { profile },
        },
        &mut budget(),
    ))?)
}

fn stage(objects: &FileObjectStore, path: &str, bytes: &[u8]) -> TestResult<ObjectRef> {
    // Legacy physical-prestage test fixture only. Production producers stage
    // through Kernel ingress before allocating objects; online GC is disabled.
    let blob = objects.put(bytes, &[], &mut budget())?.reference;
    let record = SourceRecord {
        path: path.into(),
        bytes: blob,
        byte_len: u64::try_from(bytes.len())?,
    };
    Ok(objects
        .put(&serde_json::to_vec(&record)?, &[blob], &mut budget())?
        .reference)
}

fn adopt_fixture_tree(
    catalog: &SqliteCatalog,
    objects: &FileObjectStore,
    ingress: IngressRef,
    reference: ObjectRef,
    seen: &mut std::collections::BTreeSet<ObjectRef>,
) -> TestResult {
    if !seen.insert(reference) {
        return Ok(());
    }
    let stored = objects.get(reference, &mut budget())?;
    for child in stored.registration.children.iter().copied() {
        adopt_fixture_tree(catalog, objects, ingress, child, seen)?;
    }
    catalog.reserve_inputs(ingress, std::slice::from_ref(&stored.registration))?;
    catalog.confirm_inputs(ingress, std::slice::from_ref(&stored.registration))?;
    Ok(())
}

fn stage_manifest(
    kernel: &Kernel<SqliteCatalog>,
    catalog: &SqliteCatalog,
    objects: &FileObjectStore,
    key: OperationKey,
    mutations: Vec<OwnerMutation>,
) -> TestResult<OwnerMutationSet> {
    let ingress = kernel.begin_input(key)?;
    let mut seen = std::collections::BTreeSet::new();
    for reference in mutations.iter().filter_map(|mutation| mutation.next) {
        adopt_fixture_tree(catalog, objects, ingress, reference, &mut seen)?;
    }
    Ok(kernel.stage_mutations(ingress, mutations.into_iter().map(Ok), &mut budget())?)
}

fn source(kernel: &Kernel<SqliteCatalog>, view: ViewRef) -> TestResult<SourceCut> {
    Ok(kernel.pin_view(ViewSelection::Exact(view))?.view().source)
}

fn request(
    kernel: &Kernel<SqliteCatalog>,
    catalog: &SqliteCatalog,
    objects: &FileObjectStore,
    view: ViewRef,
    source: SourceCut,
    operation_sequence: u64,
    event_sequence: u64,
    owners: Vec<OwnerMutation>,
) -> TestResult<UpdateRequest> {
    let key = operation(operation_sequence);
    let mut transition = SourceTransition {
        expected_source: source,
        owners: stage_manifest(kernel, catalog, objects, key, owners)?,
        events: Vec::new(),
    };
    transition.events.push(SourceEvent {
        producer: ProducerId([5; 16]),
        stream: StreamId([6; 16]),
        stream_epoch: 1,
        predecessor: event_sequence
            .checked_sub(1)
            .ok_or("event sequence must start at one")?,
        sequence: event_sequence,
        payload: transition.owner_digest()?,
    });
    Ok(UpdateRequest {
        operation: key,
        expected_view: view,
        kind: UpdateKind::SourceAdvance(transition),
    })
}

fn member(
    kernel: &Kernel<SqliteCatalog>,
    objects: &FileObjectStore,
    view: ViewRef,
) -> TestResult<MemberBinding> {
    let ticket = kernel.pin_view(ViewSelection::Exact(view))?;
    let mut key = b"member\0".to_vec();
    key.extend_from_slice(&spec().instance.0);
    let reference = objects
        .lookup(ticket.view().members, &key, &mut budget())?
        .ok_or("required member absent")?;
    Ok(serde_json::from_slice(
        &objects.get(reference, &mut budget())?.payload,
    )?)
}

fn hits(
    engine: &Kernel<SqliteCatalog>,
    view: ViewRef,
    objects: &FileObjectStore,
    member: &MemberBinding,
    query: &[u8],
) -> TestResult<Vec<OwnerKey>> {
    let ticket = engine.pin_view(ViewSelection::Exact(view))?;
    let page = NativeLiteralIndex.search(
        member,
        LiteralQuery {
            literal: query,
            after: None,
            limit: 100,
        },
        LiteralReadContext {
            ticket: ticket.record(),
            input: member.input,
            objects,
            maps: objects,
            budget: &mut budget(),
        },
    )?;
    Ok(page.hits.into_iter().map(|hit| hit.owner).collect())
}

fn path_owner(objects: &FileObjectStore, cut: SourceCut, path: &str) -> TestResult<OwnerKey> {
    let mut key = b"path\0".to_vec();
    key.extend_from_slice(path.as_bytes());
    let marker = objects
        .lookup(cut.paths, &key, &mut budget())?
        .ok_or("source path marker absent")?;
    Ok(serde_json::from_slice(
        &objects.get(marker, &mut budget())?.payload,
    )?)
}

fn map_count(
    objects: &FileObjectStore,
    root: quanta_index_core::index::MapRoot,
) -> TestResult<u64> {
    let mut after = None;
    let mut count = 0_u64;
    let mut work = large_fixture_budget();
    loop {
        let page = objects.scan(root, after.as_deref(), 128, &mut work)?;
        count = count
            .checked_add(u64::try_from(page.entries.len())?)
            .ok_or("map count overflow")?;
        after = page.entries.last().map(|(key, _)| key.clone());
        if !page.has_more {
            return Ok(count);
        }
        if page.entries.is_empty() {
            return Err("empty map continuation page".into());
        }
    }
}

#[test]
fn source_path_cycle_crosses_mutation_cursor_pages() -> TestResult {
    const OWNERS: u8 = 130;
    let directory = tempfile::tempdir()?;
    let (engine, objects, catalog) = kernel(directory.path())?;
    let configured = configured(&engine, &objects)?;
    let mut old_records = Vec::new();
    let mut initial = Vec::new();
    for id in 1..=OWNERS {
        let record = stage(
            &objects,
            &format!("file-{id:03}.rs"),
            format!("owner-{id:03}-old").as_bytes(),
        )?;
        old_records.push(record);
        initial.push(OwnerMutation {
            owner: owner(id),
            expected: None,
            next: Some(record),
        });
    }
    let first = published(block_on(engine.submit(
        request(
            &engine,
            &catalog,
            &objects,
            configured,
            source(&engine, configured)?,
            2,
            1,
            initial,
        )?,
        &mut budget(),
    ))?)?;
    assert_eq!(
        path_owner(&objects, source(&engine, first)?, "file-001.rs")?,
        owner(1)
    );

    let mut cycle = Vec::new();
    for id in 1..=OWNERS {
        let target = if id == OWNERS { 1 } else { id + 1 };
        let next = stage(
            &objects,
            &format!("file-{target:03}.rs"),
            format!("owner-{id:03}-new").as_bytes(),
        )?;
        let prior = old_records
            .get(usize::from(id - 1))
            .copied()
            .ok_or("prior source record absent")?;
        cycle.push(OwnerMutation {
            owner: owner(id),
            expected: Some(prior),
            next: Some(next),
        });
    }
    let second = published(block_on(engine.submit(
        request(
            &engine,
            &catalog,
            &objects,
            first,
            source(&engine, first)?,
            3,
            2,
            cycle,
        )?,
        &mut budget(),
    ))?)?;
    assert_eq!(
        path_owner(&objects, source(&engine, second)?, "file-001.rs")?,
        owner(OWNERS)
    );
    assert_eq!(
        path_owner(&objects, source(&engine, second)?, "file-129.rs")?,
        owner(128)
    );
    assert_eq!(
        path_owner(&objects, source(&engine, second)?, "file-130.rs")?,
        owner(129)
    );
    assert_eq!(
        path_owner(&objects, source(&engine, first)?, "file-001.rs")?,
        owner(1)
    );
    let member = member(&engine, &objects, second)?;
    assert_eq!(
        hits(&engine, second, &objects, &member, b"owner-129-new")?,
        vec![owner(129)]
    );
    Ok(())
}

#[test]
fn staged_mutation_pages_reject_duplicate_count_and_stale_ingress() -> TestResult {
    let directory = tempfile::tempdir()?;
    let (engine, _objects, _catalog) = kernel(directory.path())?;
    let _genesis = engine.create_workspace(workspace(), 1, &mut budget())?;
    let ingress = engine.begin_input(operation(1))?;
    let first = OwnerMutation {
        owner: owner(1),
        expected: Some(ObjectRef(Digest([71; 32]))),
        next: None,
    };
    let root = engine.stage_mutation_page(
        ingress,
        quanta_index_core::index::MapRoot(None),
        &[first.clone()],
        &mut budget(),
    )?;
    assert!(matches!(
        engine.stage_mutation_page(ingress, root, &[first.clone()], &mut budget()),
        Err(IndexError::Conflict)
    ));
    assert!(matches!(
        engine.seal_mutations(ingress, root, 2, &mut budget()),
        Err(IndexError::InvalidInput(_))
    ));
    let sealed = engine.seal_mutations(ingress, root, 1, &mut budget())?;
    assert_eq!(sealed.count, 1);
    assert_eq!(sealed.root, root);
    let replacement = engine.begin_input(operation(1))?;
    assert_ne!(ingress.fence, replacement.fence);
    assert_eq!(
        engine.stage_mutation_page(
            ingress,
            root,
            &[OwnerMutation {
                owner: owner(2),
                ..first
            }],
            &mut budget()
        ),
        Err(IndexError::FenceLost)
    );
    Ok(())
}

#[test]
fn source_only_manifest_exceeds_legacy_direct_child_limit() -> TestResult {
    const OWNERS: u32 = 4_097;
    let directory = tempfile::tempdir()?;
    let (engine, objects, _catalog) = kernel(directory.path())?;
    let genesis = engine.create_workspace(workspace(), 1, &mut large_fixture_budget())?;
    let ingress = engine.begin_input(operation(1))?;
    let mut staged_budget = large_fixture_budget();
    let mut manifest_root = quanta_index_core::index::MapRoot(None);
    let mut page = Vec::with_capacity(128);
    let mut last_record = None;
    for start in (1..=OWNERS).step_by(32) {
        let end = start.saturating_add(31).min(OWNERS);
        let captures = (start..=end)
            .map(|id| SourceInput {
                path: format!("source-{id:05}.rs"),
                bytes: b"initial contents".to_vec(),
            })
            .collect::<Vec<_>>();
        let records = engine.stage_sources(ingress, &captures, &mut staged_budget)?;
        if records.len() != captures.len() {
            return Err("source batch lost an ordinal result".into());
        }
        for (id, record) in (start..=end).zip(records) {
            if id == OWNERS {
                last_record = Some(record);
            }
            page.push(OwnerMutation {
                owner: large_owner(id),
                expected: None,
                next: Some(record),
            });
            if page.len() == 128 {
                manifest_root = engine.stage_mutation_page(
                    ingress,
                    manifest_root,
                    &page,
                    &mut staged_budget,
                )?;
                page.clear();
            }
        }
    }
    if !page.is_empty() {
        manifest_root =
            engine.stage_mutation_page(ingress, manifest_root, &page, &mut staged_budget)?;
    }
    let manifest = engine.seal_mutations(
        ingress,
        manifest_root,
        u64::from(OWNERS),
        &mut staged_budget,
    )?;
    assert_eq!(manifest.count, u64::from(OWNERS));
    let mut initial_transition = SourceTransition {
        expected_source: source(&engine, genesis)?,
        owners: manifest,
        events: Vec::new(),
    };
    initial_transition.events.push(SourceEvent {
        producer: operation(1).producer,
        stream: operation(1).stream,
        stream_epoch: 1,
        predecessor: 0,
        sequence: 1,
        payload: initial_transition.owner_digest()?,
    });
    let mut initial_budget = large_fixture_budget();
    let first = published(block_on(engine.submit(
        UpdateRequest {
            operation: operation(1),
            expected_view: genesis,
            kind: UpdateKind::SourceAdvance(initial_transition),
        },
        &mut initial_budget,
    ))?)?;
    let first_cut = source(&engine, first)?;
    assert_eq!(map_count(&objects, first_cut.owners)?, u64::from(OWNERS));
    assert_eq!(map_count(&objects, first_cut.paths)?, u64::from(OWNERS));
    let old_ticket = engine.pin_view(ViewSelection::Exact(first))?;
    let old_record = last_record.ok_or("last staged source record absent")?;

    let second_ingress = engine.begin_input(operation(2))?;
    let updated_record = engine.stage_source(
        second_ingress,
        &format!("source-{OWNERS:05}.rs"),
        b"updated contents",
        &mut staged_budget,
    )?;
    let changed = OwnerMutation {
        owner: large_owner(OWNERS),
        expected: Some(old_record),
        next: Some(updated_record),
    };
    let changed_root = engine.stage_mutation_page(
        second_ingress,
        quanta_index_core::index::MapRoot(None),
        &[changed],
        &mut staged_budget,
    )?;
    let changed_set = engine.seal_mutations(second_ingress, changed_root, 1, &mut staged_budget)?;
    let mut delta_transition = SourceTransition {
        expected_source: first_cut,
        owners: changed_set,
        events: Vec::new(),
    };
    delta_transition.events.push(SourceEvent {
        producer: operation(2).producer,
        stream: operation(2).stream,
        stream_epoch: 1,
        predecessor: 1,
        sequence: 2,
        payload: delta_transition.owner_digest()?,
    });
    let mut delta_budget = large_fixture_budget();
    let second = published(block_on(engine.submit(
        UpdateRequest {
            operation: operation(2),
            expected_view: first,
            kind: UpdateKind::SourceAdvance(delta_transition),
        },
        &mut delta_budget,
    ))?)?;
    let second_cut = source(&engine, second)?;
    assert_eq!(map_count(&objects, second_cut.owners)?, u64::from(OWNERS));
    assert_eq!(second_cut.paths, first_cut.paths);
    assert_eq!(old_ticket.view().source, first_cut);
    let unchanged_key = owner_map_key(large_owner(1));
    assert_eq!(
        objects.lookup(
            first_cut.owners,
            &unchanged_key,
            &mut large_fixture_budget()
        )?,
        objects.lookup(
            second_cut.owners,
            &unchanged_key,
            &mut large_fixture_budget()
        )?
    );
    let changed_key = owner_map_key(large_owner(OWNERS));
    assert_eq!(
        objects.lookup(first_cut.owners, &changed_key, &mut large_fixture_budget())?,
        Some(old_record)
    );
    assert_eq!(
        objects.lookup(second_cut.owners, &changed_key, &mut large_fixture_budget())?,
        Some(updated_record)
    );
    assert!(initial_budget.usage().mutations >= u64::from(OWNERS));
    assert!(delta_budget.usage().mutations < initial_budget.usage().mutations / 8);
    old_ticket.release()?;
    Ok(())
}

#[test]
fn create_edit_delete_old_reader_replay_and_cold_reopen() -> TestResult {
    let root = tempfile::tempdir()?;
    let (engine, objects, catalog) = kernel(root.path())?;
    let configured = configured(&engine, &objects)?;
    let a1 = stage(&objects, "a.rs", b"alpha needle old")?;
    let b1 = stage(&objects, "b.rs", b"beta needle shared")?;
    let initial = request(
        &engine,
        &catalog,
        &objects,
        configured,
        source(&engine, configured)?,
        2,
        1,
        vec![
            OwnerMutation {
                owner: owner(1),
                expected: None,
                next: Some(a1),
            },
            OwnerMutation {
                owner: owner(2),
                expected: None,
                next: Some(b1),
            },
        ],
    )?;
    let v1 = published(block_on(engine.submit(initial.clone(), &mut budget()))?)?;
    let old_ticket = engine.pin_view(ViewSelection::Exact(v1))?;
    let old_member = member(&engine, &objects, v1)?;
    assert_eq!(
        hits(&engine, v1, &objects, &old_member, b"needle")?,
        vec![owner(1), owner(2)]
    );

    let a2 = stage(&objects, "renamed.rs", b"alpha replacement new")?;
    let edit = request(
        &engine,
        &catalog,
        &objects,
        v1,
        source(&engine, v1)?,
        3,
        2,
        vec![OwnerMutation {
            owner: owner(1),
            expected: Some(a1),
            next: Some(a2),
        }],
    )?;
    let v2 = published(block_on(engine.submit(edit, &mut budget()))?)?;
    assert_eq!(
        hits(
            &engine,
            v2,
            &objects,
            &member(&engine, &objects, v2)?,
            b"needle"
        )?,
        vec![owner(2)]
    );
    assert_eq!(
        hits(&engine, v1, &objects, &old_member, b"needle")?,
        vec![owner(1), owner(2)]
    );
    assert_eq!(
        published(block_on(engine.submit(initial, &mut budget()))?)?,
        v1
    );

    let delete = request(
        &engine,
        &catalog,
        &objects,
        v2,
        source(&engine, v2)?,
        4,
        3,
        vec![OwnerMutation {
            owner: owner(2),
            expected: Some(b1),
            next: None,
        }],
    )?;
    let v3 = published(block_on(engine.submit(delete.clone(), &mut budget()))?)?;
    assert!(
        hits(
            &engine,
            v3,
            &objects,
            &member(&engine, &objects, v3)?,
            b"needle"
        )?
        .is_empty()
    );
    assert_eq!(old_ticket.view().identity, v1);
    old_ticket.release()?;
    drop(engine);
    drop(objects);

    let (reopened, objects, _reopened_catalog) = kernel(root.path())?;
    assert_eq!(
        reopened
            .pin_view(ViewSelection::Latest {
                workspace: workspace(),
                epoch: 1
            })?
            .view()
            .identity,
        v3
    );
    assert_eq!(
        published(block_on(reopened.submit(delete, &mut budget()))?)?,
        v3
    );
    assert_eq!(
        hits(
            &reopened,
            v3,
            &objects,
            &member(&reopened, &objects, v3)?,
            b"replacement"
        )?,
        vec![owner(1)]
    );
    Ok(())
}

#[test]
fn independent_producer_frontiers_preserve_other_contributions_and_old_view() -> TestResult {
    let root = tempfile::tempdir()?;
    let (engine, objects, catalog) = kernel(root.path())?;
    let configured = configured(&engine, &objects)?;
    let primary = (ProducerId([5; 16]), StreamId([6; 16]));
    let secondary = (ProducerId([15; 16]), StreamId([16; 16]));
    let secondary_owner = OwnerKey {
        producer: secondary.0,
        stable_id: StableOwnerId([2; 16]),
    };
    let primary_record = stage(&objects, "primary.rs", b"primary anchor")?;
    let secondary_record = stage(&objects, "secondary.rs", b"secondary marker")?;
    let mut initial = SourceTransition {
        expected_source: source(&engine, configured)?,
        owners: stage_manifest(
            &engine,
            &catalog,
            &objects,
            operation(2),
            vec![
                OwnerMutation {
                    owner: owner(1),
                    expected: None,
                    next: Some(primary_record),
                },
                OwnerMutation {
                    owner: secondary_owner,
                    expected: None,
                    next: Some(secondary_record),
                },
            ],
        )?,
        events: Vec::new(),
    };
    let initial_payload = initial.owner_digest()?;
    initial.events = [primary, secondary]
        .into_iter()
        .map(|(producer, stream)| SourceEvent {
            producer,
            stream,
            stream_epoch: 1,
            predecessor: 0,
            sequence: 1,
            payload: initial_payload,
        })
        .collect();
    let v1 = published(block_on(engine.submit(
        UpdateRequest {
            operation: operation(2),
            expected_view: configured,
            kind: UpdateKind::SourceAdvance(initial),
        },
        &mut budget(),
    ))?)?;
    let old_member = member(&engine, &objects, v1)?;
    assert_eq!(
        hits(&engine, v1, &objects, &old_member, b"anchor")?,
        vec![owner(1)]
    );
    assert_eq!(
        hits(&engine, v1, &objects, &old_member, b"marker")?,
        vec![secondary_owner]
    );

    let changed_secondary = stage(&objects, "secondary.rs", b"secondary updated")?;
    let mut secondary_delta = SourceTransition {
        expected_source: source(&engine, v1)?,
        owners: stage_manifest(
            &engine,
            &catalog,
            &objects,
            OperationKey {
                producer: secondary.0,
                stream: secondary.1,
                sequence: 41,
                ..operation(2)
            },
            vec![OwnerMutation {
                owner: secondary_owner,
                expected: Some(secondary_record),
                next: Some(changed_secondary),
            }],
        )?,
        events: Vec::new(),
    };
    secondary_delta.events.push(SourceEvent {
        producer: secondary.0,
        stream: secondary.1,
        stream_epoch: 1,
        predecessor: 1,
        sequence: 2,
        payload: secondary_delta.owner_digest()?,
    });
    let v2 = published(block_on(engine.submit(
        UpdateRequest {
            operation: OperationKey {
                producer: secondary.0,
                stream: secondary.1,
                sequence: 41,
                ..operation(2)
            },
            expected_view: v1,
            kind: UpdateKind::SourceAdvance(secondary_delta),
        },
        &mut budget(),
    ))?)?;
    let current_member = member(&engine, &objects, v2)?;
    assert_eq!(
        hits(&engine, v2, &objects, &current_member, b"anchor")?,
        vec![owner(1)]
    );
    assert_eq!(
        hits(&engine, v2, &objects, &current_member, b"updated")?,
        vec![secondary_owner]
    );
    assert!(hits(&engine, v2, &objects, &current_member, b"marker")?.is_empty());
    assert_eq!(
        hits(&engine, v1, &objects, &old_member, b"marker")?,
        vec![secondary_owner]
    );

    let frontier =
        |cut: SourceCut, producer: ProducerId, stream: StreamId| -> TestResult<SourceEvent> {
            let mut key = b"frontier\0".to_vec();
            key.extend_from_slice(&producer.0);
            key.extend_from_slice(&stream.0);
            let reference = objects
                .lookup(cut.frontiers, &key, &mut budget())?
                .ok_or("source frontier absent")?;
            Ok(serde_json::from_slice(
                &objects.get(reference, &mut budget())?.payload,
            )?)
        };
    let v2_source = source(&engine, v2)?;
    assert_eq!(frontier(v2_source, primary.0, primary.1)?.sequence, 1);
    assert_eq!(frontier(v2_source, secondary.0, secondary.1)?.sequence, 2);

    let changed_primary = stage(&objects, "primary.rs", b"primary gap")?;
    let gapped_key = OperationKey {
        sequence: 99,
        ..operation(2)
    };
    let mut gapped = SourceTransition {
        expected_source: v2_source,
        owners: stage_manifest(
            &engine,
            &catalog,
            &objects,
            gapped_key,
            vec![OwnerMutation {
                owner: owner(1),
                expected: Some(primary_record),
                next: Some(changed_primary),
            }],
        )?,
        events: Vec::new(),
    };
    gapped.events.push(SourceEvent {
        producer: primary.0,
        stream: primary.1,
        stream_epoch: 1,
        predecessor: 2,
        sequence: 3,
        payload: gapped.owner_digest()?,
    });
    assert!(matches!(
        block_on(engine.submit(
            UpdateRequest {
                operation: gapped_key,
                expected_view: v2,
                kind: UpdateKind::SourceAdvance(gapped),
            },
            &mut budget(),
        ))?,
        SubmitOutcome::Terminal(OperationStatus::Refused(Refusal::Conflict))
    ));
    assert_eq!(
        engine
            .pin_view(ViewSelection::Latest {
                workspace: workspace(),
                epoch: 1,
            })?
            .view()
            .identity,
        v2
    );
    assert_eq!(source(&engine, v2)?, v2_source);
    assert_eq!(frontier(v2_source, primary.0, primary.1)?.sequence, 1);
    assert_eq!(frontier(v2_source, secondary.0, secondary.1)?.sequence, 2);
    Ok(())
}

#[test]
fn path_collision_refuses_without_advancing_source_and_event_can_be_rebound() -> TestResult {
    let root = tempfile::tempdir()?;
    let (engine, objects, catalog) = kernel(root.path())?;
    let configured = configured(&engine, &objects)?;
    let a = stage(&objects, "same.rs", b"first")?;
    let v1 = published(block_on(engine.submit(
        request(
            &engine,
            &catalog,
            &objects,
            configured,
            source(&engine, configured)?,
            2,
            1,
            vec![OwnerMutation {
                owner: owner(1),
                expected: None,
                next: Some(a),
            }],
        )?,
        &mut budget(),
    ))?)?;
    let collision = stage(&objects, "same.rs", b"second")?;
    let original = request(
        &engine,
        &catalog,
        &objects,
        v1,
        source(&engine, v1)?,
        3,
        2,
        vec![OwnerMutation {
            owner: owner(2),
            expected: None,
            next: Some(collision),
        }],
    )?;
    let refused = block_on(engine.submit(original.clone(), &mut budget()))?;
    assert!(matches!(
        refused,
        SubmitOutcome::Terminal(OperationStatus::Refused(_))
    ));
    assert_eq!(
        engine
            .pin_view(ViewSelection::Latest {
                workspace: workspace(),
                epoch: 1
            })?
            .view()
            .identity,
        v1
    );
    let corrected = stage(&objects, "other.rs", b"second")?;
    // Rebinding retains the original captured payload; fixing invalid input
    // requires an explicit resync protocol, never replacement of e2's identity.
    let mut rebound = original;
    rebound.operation.sequence = 4;
    assert!(matches!(
        block_on(engine.submit(rebound, &mut budget()))?,
        SubmitOutcome::Terminal(OperationStatus::Refused(_))
    ));
    assert!(matches!(
        block_on(engine.submit(
            request(
                &engine,
                &catalog,
                &objects,
                v1,
                source(&engine, v1)?,
                5,
                2,
                vec![OwnerMutation {
                    owner: owner(2),
                    expected: None,
                    next: Some(corrected)
                }]
            )?,
            &mut budget()
        )),
        Err(IndexError::DigestConflict)
    ));
    Ok(())
}

#[test]
fn late_path_collision_does_not_install_earlier_owner_marker() -> TestResult {
    let root = tempfile::tempdir()?;
    let (engine, objects, catalog) = kernel(root.path())?;
    let configured = configured(&engine, &objects)?;
    let occupied = stage(&objects, "z.rs", b"original")?;
    let v1 = published(block_on(engine.submit(
        request(
            &engine,
            &catalog,
            &objects,
            configured,
            source(&engine, configured)?,
            2,
            1,
            vec![OwnerMutation {
                owner: owner(1),
                expected: None,
                next: Some(occupied),
            }],
        )?,
        &mut budget(),
    ))?)?;

    let first = stage(&objects, "a.rs", b"valid-first")?;
    let collision = stage(&objects, "z.rs", b"colliding-last")?;
    let early_owner_marker = objects
        .identify(&serde_json::to_vec(&owner(2))?, &[], &mut budget())?
        .reference;
    assert!(matches!(
        objects.get(early_owner_marker, &mut budget()),
        Err(IndexError::CorruptObject(_))
    ));
    let refused = block_on(engine.submit(
        request(
            &engine,
            &catalog,
            &objects,
            v1,
            source(&engine, v1)?,
            3,
            2,
            vec![
                OwnerMutation {
                    owner: owner(2),
                    expected: None,
                    next: Some(first),
                },
                OwnerMutation {
                    owner: owner(3),
                    expected: None,
                    next: Some(collision),
                },
            ],
        )?,
        &mut budget(),
    ))?;
    assert!(matches!(
        refused,
        SubmitOutcome::Terminal(OperationStatus::Refused(_))
    ));
    assert!(matches!(
        objects.get(early_owner_marker, &mut budget()),
        Err(IndexError::CorruptObject(_))
    ));
    assert_eq!(
        engine
            .pin_view(ViewSelection::Latest {
                workspace: workspace(),
                epoch: 1,
            })?
            .view()
            .identity,
        v1
    );
    Ok(())
}

#[test]
fn batch_boundary_preserves_all_owners_and_fixed_delta_work() -> TestResult {
    let root = tempfile::tempdir()?;
    let (engine, objects, catalog) = kernel(root.path())?;
    let configured = configured(&engine, &objects)?;
    let records = (1..=65)
        .map(|id| {
            stage(
                &objects,
                &format!("f{id}.rs"),
                format!("needle item-{id:03}").as_bytes(),
            )
            .map(|reference| OwnerMutation {
                owner: owner(id),
                expected: None,
                next: Some(reference),
            })
        })
        .collect::<TestResult<Vec<_>>>()?;
    let expected = records
        .first()
        .and_then(|value| value.next)
        .ok_or("missing first record")?;
    let initial = published(block_on(engine.submit(
        request(
            &engine,
            &catalog,
            &objects,
            configured,
            source(&engine, configured)?,
            2,
            1,
            records,
        )?,
        &mut budget(),
    ))?)?;
    // 65 distinct contributions and suffix grams cross both 64-entry batch
    // boundaries. The expected owner inventory is independent of storage.
    let original_member = member(&engine, &objects, initial)?;
    assert_eq!(
        hits(&engine, initial, &objects, &original_member, b"needle")?,
        (1..=65).map(owner).collect::<Vec<_>>()
    );
    assert_eq!(
        hits(&engine, initial, &objects, &original_member, b"item-065")?,
        vec![owner(65)]
    );
    let next = stage(&objects, "f1.rs", b"edited marker")?;
    let mut work = budget();
    let view = published(block_on(engine.submit(
        request(
            &engine,
            &catalog,
            &objects,
            initial,
            source(&engine, initial)?,
            3,
            2,
            vec![OwnerMutation {
                owner: owner(1),
                expected: Some(expected),
                next: Some(next),
            }],
        )?,
        &mut work,
    ))?)?;
    assert_eq!(
        hits(
            &engine,
            view,
            &objects,
            &member(&engine, &objects, view)?,
            b"edited"
        )?,
        vec![owner(1)]
    );
    let current_member = member(&engine, &objects, view)?;
    assert_eq!(
        hits(&engine, view, &objects, &current_member, b"needle")?,
        (2..=65).map(owner).collect::<Vec<_>>()
    );
    assert_eq!(
        hits(&engine, view, &objects, &current_member, b"item-065")?,
        vec![owner(65)]
    );
    assert_eq!(
        hits(&engine, initial, &objects, &original_member, b"needle")?,
        (1..=65).map(owner).collect::<Vec<_>>()
    );
    assert!(
        work.usage().writes < 1000,
        "one owner delta unexpectedly rewrote corpus-sized state"
    );
    Ok(())
}

/// Test transport preserves the server operation after losing the response.
/// It exercises the public SDK contract, not a daemon socket implementation.
struct LocalTransport {
    engine: Arc<Kernel<SqliteCatalog>>,
    lose_ack: std::sync::atomic::AtomicBool,
}

impl quanta_index_sdk::native_index::NativeIndexTransport for LocalTransport {
    fn submit(
        &self,
        request: UpdateRequest,
    ) -> quanta_index_sdk::native_index::NativeFuture<
        '_,
        quanta_index_core::index::OperationReply<SubmitOutcome>,
    > {
        Box::pin(async move {
            let key = request.operation;
            let result = self
                .engine
                .submit(request, &mut budget())
                .await
                .map_err(|error| {
                    quanta_index_sdk::native_index::NativeSubmitError::NotAdmitted(
                        error.to_string(),
                    )
                })?;
            if self
                .lose_ack
                .swap(false, std::sync::atomic::Ordering::SeqCst)
            {
                return Err(quanta_index_sdk::native_index::NativeSubmitError::Unknown(
                    key,
                ));
            }
            Ok(quanta_index_core::index::OperationReply {
                operation: key,
                payload: result,
            })
        })
    }

    fn status(
        &self,
        key: OperationKey,
    ) -> quanta_index_sdk::native_index::NativeFuture<
        '_,
        quanta_index_core::index::OperationReply<OperationStatus>,
    > {
        Box::pin(async move {
            let result = self.engine.status(key).map_err(|error| {
                quanta_index_sdk::native_index::NativeSubmitError::StatusTransport(
                    error.to_string(),
                )
            })?;
            Ok(quanta_index_core::index::OperationReply {
                operation: key,
                payload: result,
            })
        })
    }
}

#[test]
fn sdk_reconciles_lost_ack_with_original_key_and_receipt() -> TestResult {
    use quanta_index_sdk::native_index::{NativeIndexClient, NativeSubmitError};
    let root = tempfile::tempdir()?;
    let (engine, _objects, _catalog) = kernel(root.path())?;
    let genesis = engine.create_workspace(workspace(), 1, &mut budget())?;
    let ingress = engine.begin_input(operation(1))?;
    let profile = engine.stage_profile(
        ingress,
        &IndexProfile {
            instances: vec![spec()],
        },
        &mut budget(),
    )?;
    let engine = Arc::new(engine);
    let transport = Arc::new(LocalTransport {
        engine: engine.clone(),
        lose_ack: std::sync::atomic::AtomicBool::new(true),
    });
    let client = NativeIndexClient::new(transport);
    let request = UpdateRequest {
        operation: operation(1),
        expected_view: genesis,
        kind: UpdateKind::Reconfigure { profile },
    };
    assert_eq!(
        block_on(client.submit(request.clone())),
        Err(NativeSubmitError::Unknown(request.operation))
    );
    let OperationStatus::Published(committed) = block_on(client.status(request.operation))? else {
        return Err("lost ACK did not retain durable publication".into());
    };
    assert_eq!(published(block_on(client.submit(request))?)?, committed);
    let latest = engine.pin_view(ViewSelection::Latest {
        workspace: workspace(),
        epoch: 1,
    })?;
    assert_eq!(latest.view().identity, committed);
    Ok(())
}

#[derive(Clone)]
struct CountingOwnerAdapter(Arc<std::sync::atomic::AtomicUsize>);

impl quanta_index_core::index::ports::IndexOwnerBuildPort for CountingOwnerAdapter {
    type Candidate = quanta_index_core::index::OwnerMemberBinding;

    fn prepare_owner<'a>(
        &'a self,
        instance: &'a InstanceSpec,
        owner: OwnerKey,
        input: quanta_index_core::index::OwnerInputBinding,
        context: &'a mut quanta_index_core::index::AttemptContext<'_>,
    ) -> quanta_index_core::index::ports::IndexFuture<'a, Self::Candidate> {
        Box::pin(async move {
            let _previous = self.0.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            let payload = serde_json::to_vec(&(owner, input))
                .map_err(|error| IndexError::BackendViolation(error.to_string()))?;
            let children = input.source.into_iter().collect::<Vec<_>>();
            let artifact = context.put(&payload, &children)?;
            use sha2::{Digest as _, Sha256};
            Ok(quanta_index_core::index::OwnerMemberBinding {
                instance: instance.instance,
                owner,
                input,
                format: instance.format,
                artifact,
                logical_commitment: Digest(Sha256::digest(&payload).into()),
            })
        })
    }

    fn verify_owner<'a>(
        &'a self,
        candidate: Self::Candidate,
        _context: &'a mut quanta_index_core::index::AttemptContext<'_>,
    ) -> quanta_index_core::index::ports::IndexFuture<'a, Self::Candidate> {
        Box::pin(async move { Ok(candidate) })
    }
}

fn async_spec() -> InstanceSpec {
    InstanceSpec {
        activation: GroupActivation::AsyncCoverage,
        ..spec()
    }
}

fn owner_kernel(
    root: &std::path::Path,
    calls: Arc<std::sync::atomic::AtomicUsize>,
) -> TestResult<(
    Kernel<SqliteCatalog>,
    Arc<FileObjectStore>,
    Arc<SqliteCatalog>,
)> {
    let objects = Arc::new(FileObjectStore::open(root.join("native-objects"))?);
    let catalog = Arc::new(SqliteCatalog::open(root, Duration::from_millis(100))?);
    let mut registry = AdapterRegistry::new();
    registry.register_owner(
        AdapterDescriptor {
            kind: spec().kind,
            schema: spec().schema,
            format: spec().format,
        },
        CountingOwnerAdapter(calls),
    )?;
    let services = KernelServices::new(objects.clone(), objects.clone(), catalog.clone());
    Ok((Kernel::new(services, registry), objects, catalog))
}

fn configured_owner(
    kernel: &Kernel<SqliteCatalog>,
    _objects: &FileObjectStore,
) -> TestResult<ViewRef> {
    let genesis = kernel.create_workspace(workspace(), 1, &mut budget())?;
    let ingress = kernel.begin_input(operation(1))?;
    let profile = kernel.stage_profile(
        ingress,
        &IndexProfile {
            instances: vec![async_spec()],
        },
        &mut budget(),
    )?;
    published(block_on(kernel.submit(
        UpdateRequest {
            operation: operation(1),
            expected_view: genesis,
            kind: UpdateKind::Reconfigure { profile },
        },
        &mut budget(),
    ))?)
}

fn coverage(
    kernel: &Kernel<SqliteCatalog>,
    objects: &FileObjectStore,
    view: ViewRef,
) -> TestResult<quanta_index_core::index::CoverageBinding> {
    let ticket = kernel.pin_view(ViewSelection::Exact(view))?;
    let mut key = b"member\0".to_vec();
    key.extend_from_slice(&spec().instance.0);
    let reference = objects
        .lookup(ticket.view().coverage, &key, &mut budget())?
        .ok_or("coverage missing")?;
    Ok(serde_json::from_slice(
        &objects.get(reference, &mut budget())?.payload,
    )?)
}

fn owner_coverage(
    objects: &FileObjectStore,
    coverage: quanta_index_core::index::CoverageBinding,
    owner: OwnerKey,
) -> TestResult<quanta_index_core::index::OwnerCoverage> {
    let reference = objects
        .lookup(
            coverage.owners,
            &quanta_index_core::index::owner_map_key(owner),
            &mut budget(),
        )?
        .ok_or("owner coverage missing")?;
    Ok(serde_json::from_slice(
        &objects.get(reference, &mut budget())?.payload,
    )?)
}

fn intent_for(
    kernel: &Kernel<SqliteCatalog>,
    owner: OwnerKey,
) -> TestResult<quanta_index_core::index::WorkIntent> {
    kernel.ready_work(workspace(), 1, 128, &mut budget())?.into_iter()
        .find(|intent| matches!(intent.scope, quanta_index_core::index::WorkScope::Owner { owner: candidate, .. } if candidate == owner))
        .ok_or_else(|| "owner work missing".into())
}

fn complete_owner(
    kernel: &Kernel<SqliteCatalog>,
    view: ViewRef,
    sequence: u64,
    owner: OwnerKey,
) -> TestResult<ViewRef> {
    let claim = kernel.claim_work(&intent_for(kernel, owner)?, Duration::from_secs(120))?;
    published(block_on(kernel.submit(
        UpdateRequest {
            operation: operation(sequence),
            expected_view: view,
            kind: UpdateKind::Materialize { claim },
        },
        &mut budget(),
    ))?)
}

#[test]
fn async_coverage_and_work_manifest_cross_leaf_batch_boundary() -> TestResult {
    use quanta_index_core::index::{CoverageStatus, WorkScope};

    const OWNERS: u8 = 65;
    let root = tempfile::tempdir()?;
    let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let (engine, objects, catalog) = owner_kernel(root.path(), calls.clone())?;
    let configured = configured_owner(&engine, &objects)?;
    let mut mutations = Vec::new();
    for id in 1..=OWNERS {
        let record = stage(
            &objects,
            &format!("owner-{id:03}.rs"),
            format!("content-{id:03}").as_bytes(),
        )?;
        mutations.push(OwnerMutation {
            owner: owner(id),
            expected: None,
            next: Some(record),
        });
    }
    let view = published(block_on(engine.submit(
        request(
            &engine,
            &catalog,
            &objects,
            configured,
            source(&engine, configured)?,
            2,
            1,
            mutations,
        )?,
        &mut budget(),
    ))?)?;
    let binding = coverage(&engine, &objects, view)?;
    let CoverageStatus::Partial { missing } = binding.status else {
        return Err("65 changed owners must remain explicitly pending".into());
    };
    assert_eq!(map_count(&objects, binding.owners)?, u64::from(OWNERS));
    assert_eq!(map_count(&objects, missing)?, u64::from(OWNERS));
    let ready = engine.ready_work(workspace(), 1, 128, &mut budget())?;
    assert_eq!(ready.len(), usize::from(OWNERS));
    let ready_owners = ready
        .iter()
        .map(|intent| match intent.scope {
            WorkScope::Owner { owner, .. } => Ok(owner),
            WorkScope::ProjectionDomain { .. } => Err("unexpected projection work"),
        })
        .collect::<Result<std::collections::BTreeSet<_>, _>>()?;
    assert_eq!(ready_owners, (1..=OWNERS).map(owner).collect());
    for id in 1..=OWNERS {
        let current = owner_coverage(&objects, binding, owner(id))?;
        assert!(current.covered.is_none());
        assert!(current.searchable.is_none());
    }
    assert_eq!(calls.load(std::sync::atomic::Ordering::SeqCst), 0);

    let completed = complete_owner(&engine, view, 3, owner(1))?;
    let current = coverage(&engine, &objects, completed)?;
    let CoverageStatus::Partial { missing } = current.status else {
        return Err("64 owners must remain pending after one completion".into());
    };
    assert_eq!(map_count(&objects, missing)?, u64::from(OWNERS - 1));
    assert_eq!(
        engine.ready_work(workspace(), 1, 128, &mut budget())?.len(),
        usize::from(OWNERS - 1)
    );
    let completed_owner = owner_coverage(&objects, current, owner(1))?;
    assert_eq!(completed_owner.covered, Some(completed_owner.desired));
    assert!(completed_owner.searchable.is_some());
    assert_eq!(calls.load(std::sync::atomic::Ordering::SeqCst), 1);
    // The prior exact view remains a complete record of all 65 pending owners.
    assert_eq!(
        map_count(
            &objects,
            match coverage(&engine, &objects, view)?.status {
                CoverageStatus::Partial { missing } => missing,
                _ => return Err("old view lost pending coverage".into()),
            }
        )?,
        u64::from(OWNERS)
    );
    Ok(())
}

#[test]
fn owner_jobs_rebase_over_unrelated_edits_and_aba_reuses_durable_result() -> TestResult {
    use quanta_index_core::index::{WorkScope, WorkStatus};
    let root = tempfile::tempdir()?;
    let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let (engine, objects, catalog) = owner_kernel(root.path(), calls.clone())?;
    let configured = configured_owner(&engine, &objects)?;
    let a1 = stage(&objects, "a.rs", b"original")?;
    let b1 = stage(&objects, "b.rs", b"unrelated")?;
    let v1 = published(block_on(engine.submit(
        request(
            &engine,
            &catalog,
            &objects,
            configured,
            source(&engine, configured)?,
            2,
            1,
            vec![
                OwnerMutation {
                    owner: owner(1),
                    expected: None,
                    next: Some(a1),
                },
                OwnerMutation {
                    owner: owner(2),
                    expected: None,
                    next: Some(b1),
                },
            ],
        )?,
        &mut budget(),
    ))?)?;
    let a_intent = intent_for(&engine, owner(1))?;
    let a_claim = engine.claim_work(&a_intent, Duration::from_secs(120))?;
    let b_intent = intent_for(&engine, owner(2))?;
    let b_claim = engine.claim_work(&b_intent, Duration::from_secs(120))?;
    let b2 = stage(&objects, "b.rs", b"edited unrelated")?;
    let v2 = published(block_on(engine.submit(
        request(
            &engine,
            &catalog,
            &objects,
            v1,
            source(&engine, v1)?,
            3,
            2,
            vec![OwnerMutation {
                owner: owner(2),
                expected: Some(b1),
                next: Some(b2),
            }],
        )?,
        &mut budget(),
    ))?)?;
    assert_eq!(engine.work_status(&b_intent)?, WorkStatus::Superseded);
    assert_eq!(engine.work_status(&a_intent)?, WorkStatus::Claimed(a_claim));
    let v3 = published(block_on(engine.submit(
        UpdateRequest {
            operation: operation(4),
            expected_view: v1,
            kind: UpdateKind::Materialize { claim: a_claim },
        },
        &mut budget(),
    ))?)?;
    assert!(v3.sequence > v2.sequence);
    let current = owner_coverage(&objects, coverage(&engine, &objects, v3)?, owner(1))?;
    assert_eq!(current.covered, Some(current.desired));
    let stale = block_on(engine.submit(
        UpdateRequest {
            operation: operation(5),
            expected_view: v1,
            kind: UpdateKind::Materialize { claim: b_claim },
        },
        &mut budget(),
    ))?;
    assert!(matches!(
        stale,
        SubmitOutcome::Terminal(OperationStatus::Refused(_))
    ));
    assert_eq!(calls.load(std::sync::atomic::Ordering::SeqCst), 1);
    let a2 = stage(&objects, "a.rs", b"replacement")?;
    let v4 = published(block_on(engine.submit(
        request(
            &engine,
            &catalog,
            &objects,
            v3,
            source(&engine, v3)?,
            6,
            3,
            vec![OwnerMutation {
                owner: owner(1),
                expected: Some(a1),
                next: Some(a2),
            }],
        )?,
        &mut budget(),
    ))?)?;
    let v5 = complete_owner(&engine, v4, 7, owner(1))?;
    assert_eq!(calls.load(std::sync::atomic::Ordering::SeqCst), 2);
    let v6 = published(block_on(engine.submit(
        request(
            &engine,
            &catalog,
            &objects,
            v5,
            source(&engine, v5)?,
            8,
            4,
            vec![OwnerMutation {
                owner: owner(1),
                expected: Some(a2),
                next: Some(a1),
            }],
        )?,
        &mut budget(),
    ))?)?;
    let restored_intent = intent_for(&engine, owner(1))?;
    assert_eq!(restored_intent, a_intent);
    assert!(matches!(restored_intent.scope, WorkScope::Owner { .. }));
    let v7 = complete_owner(&engine, v6, 9, owner(1))?;
    assert_eq!(
        calls.load(std::sync::atomic::Ordering::SeqCst),
        2,
        "ABA repeated expensive owner derivation"
    );
    let restored = owner_coverage(&objects, coverage(&engine, &objects, v7)?, owner(1))?;
    assert_eq!(restored.desired, current.desired);
    assert_eq!(restored.covered, Some(restored.desired));
    let old = owner_coverage(&objects, coverage(&engine, &objects, v1)?, owner(1))?;
    assert_eq!(
        old.covered, None,
        "materialization changed old immutable view"
    );
    Ok(())
}

#[test]
fn interrupted_resume_does_not_rotate_a_pending_operation_fence() -> TestResult {
    let directory = tempfile::tempdir()?;
    let (engine, objects, catalog) = kernel(directory.path())?;
    let configured = configured(&engine, &objects)?;
    let record = stage(&objects, "resume.rs", b"pending recovery")?;
    let request = request(
        &engine,
        &catalog,
        &objects,
        configured,
        source(&engine, configured)?,
        2,
        1,
        vec![OwnerMutation {
            owner: owner(1),
            expected: None,
            next: Some(record),
        }],
    )?;
    let key = request.operation;
    // Use the public durable admission boundary to leave real Pending custody.
    // A low write budget may legitimately produce a terminal refusal instead.
    let UpdateKind::SourceAdvance(transition) = &request.kind else {
        return Err("expected source transition fixture".into());
    };
    let ingress = engine.begin_input(key)?;
    let stored_request = objects.put(
        &serde_json::to_vec(&request)?,
        &transition.owners.object_children(),
        &mut budget(),
    )?;
    catalog.reserve_inputs(ingress, std::slice::from_ref(&stored_request))?;
    catalog.confirm_inputs(ingress, std::slice::from_ref(&stored_request))?;
    catalog.seal_inputs(ingress, stored_request.reference)?;
    assert!(matches!(
        catalog.admit(
            ImmutableOperation {
                key,
                request: stored_request.reference
            },
            ingress,
            &transition.events,
            &mut budget(),
        )?,
        AdmissionOutcome::New(_)
    ));
    let limits = WorkLimits {
        reads: 1_000_000,
        writes: 1,
        bytes: 512 * 1024 * 1024,
        mutations: 1_000_000,
    };
    let original = engine.status(key)?;
    assert!(matches!(original, OperationStatus::Pending(_)));

    let mut expired = WorkBudget::new(limits, Instant::now());
    assert_eq!(
        block_on(engine.resume(key, &mut expired)),
        Err(IndexError::ResourceLimited)
    );
    assert_eq!(engine.status(key)?, original);

    let request_budget =
        quanta_index_core::RequestBudgetV1::until(Instant::now() + Duration::from_secs(30));
    request_budget.cancel_handle().cancel();
    let mut cancelled = WorkBudget::with_request(limits, request_budget);
    assert_eq!(
        block_on(engine.resume(key, &mut cancelled)),
        Err(IndexError::Cancelled)
    );
    assert_eq!(engine.status(key)?, original);
    Ok(())
}

#[test]
fn expired_work_claim_terminates_old_operation_and_recovery_can_complete() -> TestResult {
    use quanta_index_core::index::Refusal;
    let root = tempfile::tempdir()?;
    let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let (engine, objects, catalog) = owner_kernel(root.path(), calls.clone())?;
    let configured = configured_owner(&engine, &objects)?;
    let a = stage(&objects, "a.rs", b"lease test")?;
    let view = published(block_on(engine.submit(
        request(
            &engine,
            &catalog,
            &objects,
            configured,
            source(&engine, configured)?,
            2,
            1,
            vec![OwnerMutation {
                owner: owner(1),
                expected: None,
                next: Some(a),
            }],
        )?,
        &mut budget(),
    ))?)?;
    let intent = intent_for(&engine, owner(1))?;
    let expired = engine.claim_work(&intent, Duration::from_millis(1))?;
    std::thread::sleep(Duration::from_millis(5));
    let old_operation = operation(3);
    let outcome = block_on(engine.submit(
        UpdateRequest {
            operation: old_operation,
            expected_view: view,
            kind: UpdateKind::Materialize { claim: expired },
        },
        &mut budget(),
    ))?;
    assert_eq!(
        outcome,
        SubmitOutcome::Terminal(OperationStatus::Refused(Refusal::FenceLost))
    );
    assert_eq!(
        block_on(engine.resume(old_operation, &mut budget()))?,
        outcome
    );
    assert_eq!(calls.load(std::sync::atomic::Ordering::SeqCst), 0);
    let recovered = engine.claim_work(&intent, Duration::from_secs(120))?;
    assert_ne!(expired.fence, recovered.fence);
    let result = block_on(engine.submit(
        UpdateRequest {
            operation: operation(4),
            expected_view: view,
            kind: UpdateKind::Materialize { claim: recovered },
        },
        &mut budget(),
    ))?;
    let _published = published(result)?;
    assert_eq!(calls.load(std::sync::atomic::Ordering::SeqCst), 1);
    Ok(())
}

fn file_member(
    engine: &Kernel<SqliteCatalog>,
    objects: &FileObjectStore,
    view: ViewRef,
) -> TestResult<MemberBinding> {
    let ticket = engine.pin_view(ViewSelection::Exact(view))?;
    let mut key = b"member\0".to_vec();
    key.extend_from_slice(&file_spec().instance.0);
    let reference = objects
        .lookup(ticket.view().members, &key, &mut budget())?
        .ok_or("file member missing")?;
    Ok(serde_json::from_slice(
        &objects.get(reference, &mut budget())?.payload,
    )?)
}

#[test]
fn required_lexical_and_filemap_publish_one_view_and_prefix_pagination_is_exact() -> TestResult {
    let root = tempfile::tempdir()?;
    let (engine, objects, catalog) = kernel(root.path())?;
    let configured = configured(&engine, &objects)?;
    let a = stage(&objects, "a", b"needle")?;
    let ab = stage(&objects, "ab", b"needle")?;
    let z = stage(&objects, "z", b"irrelevant")?;
    let view = published(block_on(engine.submit(
        request(
            &engine,
            &catalog,
            &objects,
            configured,
            source(&engine, configured)?,
            2,
            1,
            vec![
                OwnerMutation {
                    owner: owner(1),
                    expected: None,
                    next: Some(a),
                },
                OwnerMutation {
                    owner: owner(2),
                    expected: None,
                    next: Some(ab),
                },
                OwnerMutation {
                    owner: owner(3),
                    expected: None,
                    next: Some(z),
                },
            ],
        )?,
        &mut budget(),
    ))?)?;
    let ticket = engine.pin_view(ViewSelection::Exact(view))?;
    let file_binding = file_member(&engine, &objects, view)?;
    let mut work = budget();
    let mut context = FileMapReadContext {
        ticket: ticket.record(),
        objects: objects.as_ref(),
        maps: objects.as_ref(),
        budget: &mut work,
    };
    let first = query_paths(
        &FilePathQuery {
            member: &file_binding,
            prefix: "a",
            after: None,
            limit: 1,
        },
        &mut context,
    )?;
    assert_eq!(
        first
            .entries
            .iter()
            .map(|entry| entry.path.as_str())
            .collect::<Vec<_>>(),
        vec!["a"]
    );
    assert!(first.has_more);
    let second = query_paths(
        &FilePathQuery {
            member: &file_binding,
            prefix: "a",
            after: Some("a"),
            limit: 1,
        },
        &mut context,
    )?;
    assert_eq!(
        second
            .entries
            .iter()
            .map(|entry| entry.path.as_str())
            .collect::<Vec<_>>(),
        vec!["ab"]
    );
    assert!(!second.has_more);
    let missing = query_paths(
        &FilePathQuery {
            member: &file_binding,
            prefix: "missing/",
            after: None,
            limit: 1,
        },
        &mut context,
    )?;
    assert!(missing.entries.is_empty());
    assert!(!missing.has_more);
    assert_eq!(
        hits(
            &engine,
            view,
            &objects,
            &member(&engine, &objects, view)?,
            b"needle"
        )?,
        vec![owner(1), owner(2)]
    );
    Ok(())
}

/// A terminal retry is a digest/status lookup, not a replay of retained bytes.
/// Fault injection removes only the request object, leaving published roots.
#[test]
fn terminal_replay_survives_pruned_physical_request() -> TestResult {
    use std::fmt::Write as _;
    let temp = tempfile::tempdir()?;
    let (engine, objects, catalog) = kernel(temp.path())?;
    let initial = configured(&engine, &objects)?;
    let record = stage(&objects, "a.rs", b"immutable needle")?;
    let update = request(
        &engine,
        &catalog,
        &objects,
        initial,
        source(&engine, initial)?,
        2,
        1,
        vec![OwnerMutation {
            owner: owner(1),
            expected: None,
            next: Some(record),
        }],
    )?;
    let view = published(block_on(engine.submit(update.clone(), &mut budget()))?)?;
    let request_ref = objects
        .identify(
            &serde_json::to_vec(&update)?,
            &match &update.kind {
                UpdateKind::SourceAdvance(transition) => transition.owners.object_children(),
                _ => return Err("expected source request fixture".into()),
            },
            &mut budget(),
        )?
        .reference;
    let mut name = String::with_capacity(64);
    for byte in (request_ref.0).0 {
        write!(&mut name, "{byte:02x}")?;
    }
    let shard = format!("{:02x}", (request_ref.0).0.first().ok_or("digest empty")?);
    std::fs::remove_file(
        temp.path()
            .join("native-objects/objects")
            .join(shard)
            .join(name),
    )?;
    assert_eq!(
        published(block_on(engine.submit(update.clone(), &mut budget()))?)?,
        view
    );
    let mut conflict = update;
    if let UpdateKind::SourceAdvance(transition) = &mut conflict.kind {
        transition.owners = OwnerMutationSet {
            root: quanta_index_core::index::MapRoot(None),
            count: 0,
            digest: quanta_index_core::index::OwnerMutationDigest::default().finish(),
        };
        let payload = transition.owner_digest()?;
        for event in &mut transition.events {
            event.payload = payload;
        }
    }
    assert_eq!(
        block_on(engine.submit(conflict, &mut budget())),
        Err(IndexError::DigestConflict)
    );
    assert_eq!(
        hits(
            &engine,
            view,
            &objects,
            &member(&engine, &objects, view)?,
            b"needle"
        )?,
        vec![owner(1)]
    );
    drop(engine);
    drop(objects);
    let (reopened, objects, _reopened_catalog) = kernel(temp.path())?;
    assert_eq!(
        reopened.status(operation(2))?,
        OperationStatus::Published(view)
    );
    assert_eq!(
        hits(
            &reopened,
            view,
            &objects,
            &member(&reopened, &objects, view)?,
            b"needle"
        )?,
        vec![owner(1)]
    );
    Ok(())
}
