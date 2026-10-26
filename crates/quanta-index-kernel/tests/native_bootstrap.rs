//! Cold installation of three adapters over an existing immutable source cut.
//! Fixed query gold and publication failure checks; not a latency benchmark.
#![expect(
    clippy::disallowed_methods,
    reason = "Integration fixtures synchronously drive the async kernel API; this is not a production application boundary"
)]
#![expect(
    clippy::panic_in_result_fn,
    reason = "Test Results propagate fixture IO failures while assertions remain independent test oracles"
)]
use std::{
    error::Error,
    path::Path,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    time::{Duration, Instant},
};

use futures::executor::block_on;
use quanta_index_catalog::SqliteCatalog;
use quanta_index_contract::{LqExpr, LqLeaf, LqQuery, LqSpan};
use quanta_index_core::index::{
    AttemptContext, BaseMember, CollectionId, DeltaInput, Digest, GroupActivation, IndexError,
    IndexInstanceId, IndexKind, IndexProfile, InputDomain, InstanceSpec, MemberBinding,
    OperationKey, OperationStatus, OwnerKey, OwnerMutation, PlanContext, PlanDecision, ProducerId,
    PublicationGroupId, Refusal, SchemaId, SlotId, SourceDelta, SourceEvent, SourceInput,
    SourceTextAdmission, SourceTransition, StableOwnerId, StorageFormatId, StreamId, SubmitOutcome,
    TenantId, TicketRecord, UpdateKind, UpdateRequest, ViewRef, ViewSelection, WorkBudget,
    WorkLimits, WorkspaceKey, WorktreeId,
    ports::{ImmutableObjectPort, IndexBuildPort, IndexFuture, PersistentMapPort},
};
use quanta_index_kernel::{AdapterDescriptor, AdapterRegistry, Kernel, KernelServices};
use quanta_index_lexical::native::{
    Bm25Policy, CaseMode, LiteralQuery, LiteralReadContext, NativeLiteralIndex, NativeTextIndex,
    TextReadContext, TextRecipe,
};
use quanta_index_repomap::native::{FileMapReadContext, FilePathQuery, NativeFileMap, query_paths};
use quanta_index_storage::{FileObjectStore, FileScratchSpace};

type TestResult<T = ()> = Result<T, Box<dyn Error>>;
const ADMISSION: Digest = Digest([31; 32]);

fn budget() -> TestResult<WorkBudget> {
    Ok(WorkBudget::new(
        WorkLimits {
            reads: 2_000_000,
            writes: 2_000_000,
            bytes: 1024 * 1024 * 1024,
            mutations: 2_000_000,
        },
        Instant::now()
            .checked_add(Duration::from_secs(120))
            .ok_or("fixture deadline overflow")?,
    ))
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

fn owner(id: u16) -> OwnerKey {
    OwnerKey {
        producer: ProducerId([5; 16]),
        stable_id: StableOwnerId(u128::from(id).to_be_bytes()),
    }
}

fn spec(id: u8, policy: Digest) -> TestResult<InstanceSpec> {
    Ok(InstanceSpec {
        instance: IndexInstanceId([id; 16]),
        kind: IndexKind([id.checked_add(1).ok_or("fixture kind overflow")?; 16]),
        group: PublicationGroupId([9; 16]),
        activation: GroupActivation::CoreRequired,
        input_domain: InputDomain::SourceLocal,
        schema: SchemaId([10; 16]),
        policy,
        format: StorageFormatId([12; 16]),
    })
}

fn specs() -> TestResult<[InstanceSpec; 3]> {
    Ok([
        spec(7, Digest([11; 32]))?,
        spec(17, Digest([11; 32]))?,
        spec(27, TextRecipe::new(ADMISSION).output_policy())?,
    ])
}

fn descriptor(spec: &InstanceSpec) -> AdapterDescriptor {
    AdapterDescriptor {
        kind: spec.kind,
        schema: spec.schema,
        format: spec.format,
    }
}

struct CountedLiteral(Arc<AtomicUsize>);

impl IndexBuildPort for CountedLiteral {
    type Delta = SourceDelta;
    type Plan = <NativeLiteralIndex as IndexBuildPort>::Plan;
    type Candidate = <NativeLiteralIndex as IndexBuildPort>::Candidate;

    fn resource_demand(&self) -> quanta_index_core::index::effort::ResourceNeed {
        NativeLiteralIndex.resource_demand()
    }

    fn validate_instance(
        &self,
        instance: &quanta_index_core::index::InstanceSpec,
    ) -> Result<(), IndexError> {
        NativeLiteralIndex.validate_instance(instance)
    }

    fn plan(
        &self,
        base: BaseMember,
        input: DeltaInput<SourceDelta>,
        context: &PlanContext,
        budget: &mut WorkBudget,
    ) -> Result<PlanDecision<Self::Plan>, IndexError> {
        NativeLiteralIndex.plan(base, input, context, budget)
    }

    fn prepare<'a>(
        &'a self,
        plan: Self::Plan,
        context: &'a mut AttemptContext<'_>,
    ) -> IndexFuture<'a, Self::Candidate> {
        let _previous = self.0.fetch_add(1, Ordering::Relaxed);
        NativeLiteralIndex.prepare(plan, context)
    }

    fn verify_and_seal<'a>(
        &'a self,
        candidate: Self::Candidate,
        context: &'a mut AttemptContext<'_>,
    ) -> IndexFuture<'a, MemberBinding> {
        NativeLiteralIndex.verify_and_seal(candidate, context)
    }
}

fn open_engine(
    root: &Path,
    literal_builds: Arc<AtomicUsize>,
) -> TestResult<(Kernel<SqliteCatalog>, Arc<FileObjectStore>)> {
    let objects = Arc::new(FileObjectStore::open(root.join("objects"))?);
    let catalog = Arc::new(SqliteCatalog::open(root, Duration::from_millis(100))?);
    let mut registry = AdapterRegistry::new();
    let [literal, paths, text] = specs()?;
    registry.register(
        descriptor(&literal),
        CountedLiteral(literal_builds),
        |delta| Ok(delta.clone()),
    )?;
    registry.register(descriptor(&paths), NativeFileMap, |delta| Ok(delta.clone()))?;
    registry.register(
        descriptor(&text),
        NativeTextIndex::new(TextRecipe::new(ADMISSION)),
        |delta| Ok(delta.clone()),
    )?;
    let wrong_recipe = TextRecipe::new(Digest([32; 32]));
    registry.register(
        descriptor(&spec(37, wrong_recipe.output_policy())?),
        NativeTextIndex::new(wrong_recipe),
        |delta| Ok(delta.clone()),
    )?;
    let services = KernelServices::new(
        objects.clone(),
        objects.clone(),
        catalog,
        Arc::new(FileScratchSpace::open_standalone(
            root.join("scratch"),
            16 * 1024 * 1024 * 1024,
        )?),
        Duration::from_secs(120),
    )?;
    Ok((Kernel::new(services, registry), objects))
}

fn published(outcome: SubmitOutcome) -> TestResult<ViewRef> {
    match outcome {
        SubmitOutcome::Terminal(OperationStatus::Published(view)) => Ok(view),
        other @ (SubmitOutcome::Accepted(_)
        | SubmitOutcome::Terminal(_)
        | SubmitOutcome::Unknown(_)) => Err(format!("expected publication, got {other:?}").into()),
    }
}

fn reconfigure(
    engine: &Kernel<SqliteCatalog>,
    base: ViewRef,
    sequence: u64,
    instances: Vec<InstanceSpec>,
) -> TestResult<SubmitOutcome> {
    let key = operation(sequence);
    let ingress = engine.begin_input(key)?;
    let profile = engine.stage_profile(ingress, &IndexProfile { instances }, &mut budget()?)?;
    Ok(block_on(engine.submit(
        UpdateRequest {
            operation: key,
            expected_view: base,
            kind: UpdateKind::Reconfigure { profile },
        },
        &mut budget()?,
    ))?)
}

fn binding(
    objects: &FileObjectStore,
    ticket: &TicketRecord,
    instance: IndexInstanceId,
) -> TestResult<MemberBinding> {
    let mut key = b"member\0".to_vec();
    key.extend_from_slice(&instance.0);
    let reference = objects
        .lookup_object(ticket.view.members, &key, &mut budget()?)?
        .ok_or("cold-built member missing")?;
    Ok(serde_json::from_slice(
        &objects.get(reference, &mut budget()?)?.payload,
    )?)
}

fn assert_gold(
    engine: &Kernel<SqliteCatalog>,
    objects: &FileObjectStore,
    view: ViewRef,
) -> TestResult {
    let ticket = engine.pin_view(ViewSelection::Exact(view), &budget()?)?;
    let [literal_spec, path_spec, text_spec] = specs()?;
    let literal = binding(objects, ticket.record(), literal_spec.instance)?;
    let mut after = None;
    let mut literal_owners = Vec::new();
    loop {
        let page = NativeLiteralIndex.search(
            &literal,
            LiteralQuery {
                literal: b"Needle",
                after: after.as_deref(),
                limit: 64,
            },
            LiteralReadContext {
                ticket: ticket.record(),
                input: literal.input,
                objects,
                maps: objects,
                budget: &mut budget()?,
            },
        )?;
        literal_owners.extend(page.hits.into_iter().map(|hit| hit.owner));
        if !page.has_more {
            break;
        }
        after = Some(page.after.ok_or("literal continuation absent")?);
    }
    assert_eq!(literal_owners, (1..=129).map(owner).collect::<Vec<_>>());
    let paths = binding(objects, ticket.record(), path_spec.instance)?;
    let mut after = None;
    let mut actual_paths = Vec::new();
    loop {
        let page = query_paths(
            &FilePathQuery {
                member: &paths,
                prefix: "",
                after: after.as_deref(),
                limit: 64,
            },
            &mut FileMapReadContext {
                ticket: ticket.record(),
                objects,
                maps: objects,
                budget: &mut budget()?,
            },
        )?;
        actual_paths.extend(
            page.entries
                .iter()
                .map(|entry| (entry.path.clone(), entry.owner)),
        );
        if !page.has_more {
            break;
        }
        after = Some(
            page.entries
                .last()
                .ok_or("path continuation empty")?
                .path
                .clone(),
        );
    }
    let mut expected_paths = (1..=129)
        .map(|id| (format!("src/{id:03}.rs"), owner(id)))
        .collect::<Vec<_>>();
    expected_paths.extend([
        ("src/empty.rs".into(), owner(130)),
        ("src/excluded.bin".into(), owner(131)),
    ]);
    assert_eq!(actual_paths, expected_paths);
    let text = binding(objects, ticket.record(), text_spec.instance)?;
    let reader = NativeTextIndex::new(TextRecipe::new(ADMISSION));
    let stats = reader.inspect_stats(
        &text,
        TextReadContext {
            ticket: ticket.record(),
            objects,
            maps: objects,
            budget: &mut budget()?,
        },
    )?;
    assert_eq!((stats.docs, stats.indexed_tokens), (130, 259));
    assert_eq!(
        reader.inspect_term_df(
            &text,
            CaseMode::Folded,
            "CAFÉ",
            TextReadContext {
                ticket: ticket.record(),
                objects,
                maps: objects,
                budget: &mut budget()?,
            }
        )?,
        129
    );
    let mut query = LqQuery::empty(LqSpan::synthetic(0));
    query.expr = LqExpr::Leaf(LqLeaf::Keyword("needle".into()));
    let plan = quanta_index_lexical::planner::LexicalPlanner::plan(&query)?;
    let ranking = reader.search_content(
        &text,
        &plan,
        &quanta_index_contract::QueryConstraintSetV1::unconstrained(),
        3,
        Bm25Policy { k1: 1.2, b: 0.75 },
        TextReadContext {
            ticket: ticket.record(),
            objects,
            maps: objects,
            budget: &mut budget()?,
        },
    )?;
    assert_eq!(ranking.view, view);
    assert_eq!(ranking.matched, 129);
    assert_eq!(
        ranking.hits.iter().map(|hit| hit.owner).collect::<Vec<_>>(),
        vec![owner(129), owner(1), owner(2)]
    );
    let [first, second, third] = ranking.hits.as_slice() else {
        return Err("expected three ranked hits".into());
    };
    assert!(first.score.is_finite() && first.score > 0.0);
    assert!(first.score > second.score);
    assert_eq!(second.score.to_bits(), third.score.to_bits());
    ticket.release()?;
    Ok(())
}

#[test]
fn existing_corpus_installs_all_adapters_atomically_across_pages_and_reopens() -> TestResult {
    let root = tempfile::tempdir()?;
    let literal_builds = Arc::new(AtomicUsize::new(0));
    let (engine, objects) = open_engine(root.path(), literal_builds.clone())?;
    let genesis = engine.create_workspace(workspace(), 1, &mut budget()?)?;
    let ingress = engine.begin_input(operation(1))?;
    let admitted = SourceTextAdmission::Utf8 { policy: ADMISSION };
    let mut inputs = (1..=129)
        .map(|id| SourceInput {
            path: format!("src/{id:03}.rs"),
            bytes: if id == 129 {
                "Needle Needle café"
            } else {
                "Needle café"
            }
            .as_bytes()
            .to_vec(),
            text: admitted,
        })
        .collect::<Vec<_>>();
    inputs.extend([
        SourceInput {
            path: "src/empty.rs".into(),
            bytes: Vec::new(),
            text: admitted,
        },
        SourceInput {
            path: "src/excluded.bin".into(),
            bytes: vec![0xff],
            text: SourceTextAdmission::Excluded,
        },
    ]);
    let mut references = Vec::new();
    for page in inputs.chunks(32) {
        references.extend(engine.stage_sources(ingress, page, &mut budget()?)?);
    }
    let mutations = engine.stage_mutations(
        ingress,
        (1..=131).zip(references).map(|(id, reference)| {
            Ok(OwnerMutation {
                owner: owner(id),
                expected: None,
                next: Some(reference),
            })
        }),
        &mut budget()?,
    )?;
    let before = engine.pin_view(ViewSelection::Exact(genesis), &budget()?)?;
    let mut transition = SourceTransition {
        expected_source: before.view().source,
        owners: mutations,
        events: Vec::new(),
    };
    transition.events.push(SourceEvent {
        producer: operation(1).producer,
        stream: operation(1).stream,
        stream_epoch: 1,
        predecessor: 0,
        sequence: 1,
        payload: transition.owner_digest()?,
    });
    before.release()?;
    let source_only = published(block_on(engine.submit(
        UpdateRequest {
            operation: operation(1),
            expected_view: genesis,
            kind: UpdateKind::SourceAdvance(transition),
        },
        &mut budget()?,
    ))?)?;
    let old = engine.pin_view(ViewSelection::Exact(source_only), &budget()?)?;
    assert!(old.view().members.0.is_none());
    let [literal, paths, text] = specs()?;
    let wrong_text = spec(37, TextRecipe::new(Digest([32; 32])).output_policy())?;
    assert_eq!(
        reconfigure(
            &engine,
            source_only,
            2,
            vec![literal.clone(), paths.clone(), wrong_text]
        )?,
        SubmitOutcome::Terminal(OperationStatus::Refused(Refusal::InvalidInput))
    );
    let refused = engine.pin_view(
        ViewSelection::Latest {
            workspace: workspace(),
            epoch: 1,
        },
        &budget()?,
    )?;
    assert_eq!(refused.view().identity, source_only);
    assert!(refused.view().members.0.is_none());
    refused.release()?;
    assert_eq!(literal_builds.load(Ordering::Relaxed), 1);
    let literal_only = published(reconfigure(&engine, source_only, 3, vec![literal.clone()])?)?;
    let literal_ticket = engine.pin_view(ViewSelection::Exact(literal_only), &budget()?)?;
    let initial_literal = binding(&objects, literal_ticket.record(), literal.instance)?;
    assert_eq!(literal_builds.load(Ordering::Relaxed), 2);
    let installed = published(reconfigure(
        &engine,
        literal_only,
        4,
        vec![literal.clone(), paths.clone(), text.clone()],
    )?)?;
    let current = engine.pin_view(ViewSelection::Exact(installed), &budget()?)?;
    assert_eq!(current.view().source, old.view().source);
    assert_eq!(
        binding(&objects, current.record(), literal.instance)?,
        initial_literal
    );
    assert_eq!(literal_builds.load(Ordering::Relaxed), 2);
    current.release()?;
    old.validate()?;
    assert!(old.view().members.0.is_none());
    assert_gold(&engine, &objects, installed)?;
    // An explicitly changed required policy rebuilds from the same source cut.
    let changed_literal = InstanceSpec {
        policy: Digest([51; 32]),
        ..literal
    };
    let rebuilt = published(reconfigure(
        &engine,
        installed,
        5,
        vec![changed_literal, paths, text],
    )?)?;
    assert_eq!(literal_builds.load(Ordering::Relaxed), 3);
    assert_gold(&engine, &objects, rebuilt)?;
    assert_gold(&engine, &objects, installed)?;
    literal_ticket.validate()?;
    assert_eq!(
        binding(&objects, literal_ticket.record(), initial_literal.instance)?,
        initial_literal
    );
    literal_ticket.release()?;
    old.release()?;
    drop(engine);
    drop(objects);
    let (reopened, objects) = open_engine(root.path(), literal_builds.clone())?;
    assert_gold(&reopened, &objects, rebuilt)?;
    assert_gold(&reopened, &objects, installed)?;
    assert_eq!(literal_builds.load(Ordering::Relaxed), 3);
    Ok(())
}
