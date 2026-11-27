//! A Text member whose root was written by a retired wire version is stale,
//! not corrupt.
//!
//! The previous release's layered writer is stood in by an adapter that
//! publishes a `QTXROOT3` root exactly as that release encoded it. The
//! current Text adapter then reports the view as rebuild-needed (typed
//! `RebuildRequired`), the store-level member scrub finds nothing to
//! quarantine, and replacing the Text instance rebuilds a readable member.

use super::*;
use quanta_index_contract::{LqExpr, LqLeaf, LqQuery, LqSpan, QueryConstraintSetV1};
use quanta_index_core::index::{
    AttemptContext, BaseMember, DeltaInput, PlanContext, PlanDecision, Refusal, SourceDelta,
    SourceTextAdmission,
    ports::{IndexBuildPort, IndexFuture},
};
use quanta_index_lexical::native::{Bm25Policy, NativeTextIndex, TextReadContext, TextRecipe};
use quanta_index_lexical::planner::LexicalPlanner;

fn recipe() -> TextRecipe {
    TextRecipe::segmented_layered(Digest([31; 32]))
}

fn text_spec(instance: u8) -> InstanceSpec {
    InstanceSpec {
        instance: IndexInstanceId([instance; 16]),
        kind: IndexKind([28; 16]),
        policy: recipe().output_policy(),
        ..spec()
    }
}

/// The retired layered root encoding and its v3 logical commitment.
///
/// Magic, policy, the three `SourceCut` refs (frontiers included), no path
/// postings, zero path statistics and no rows; children are the present
/// source refs.
fn retired_layered_root(policy: Digest, source: SourceCut) -> (Vec<u8>, Vec<ObjectRef>, Digest) {
    let mut payload = b"QTXROOT3".to_vec();
    payload.extend_from_slice(&policy.0);
    let mut logical = Sha256::new();
    logical.update(b"qidx.native.text.segment.layered.logical.v3\0");
    logical.update(policy.0);
    for root in [source.owners, source.paths, source.frontiers] {
        if let Some(reference) = root.0 {
            payload.push(1);
            payload.extend_from_slice(&reference.0.0);
            logical.update([1]);
            logical.update(reference.0.0);
        } else {
            payload.push(0);
            logical.update([0]);
        }
    }
    payload.push(0);
    payload.extend_from_slice(&[0; 16]);
    payload.extend_from_slice(&[0, 0]);
    for zero in [0_u64; 4] {
        logical.update(zero.to_be_bytes());
    }
    let children = [source.owners.0, source.paths.0, source.frontiers.0]
        .into_iter()
        .flatten()
        .collect();
    (payload, children, Digest(logical.finalize().into()))
}

/// Stands in for the previous binary's layered Text writer.
struct RetiredLayeredWriter;

impl IndexBuildPort for RetiredLayeredWriter {
    type Delta = SourceDelta;
    type Plan = PlanContext;
    type Candidate = MemberBinding;

    fn resource_demand(&self) -> ResourceNeed {
        ResourceNeed::default()
    }

    fn validate_instance(&self, _instance: &InstanceSpec) -> Result<(), IndexError> {
        Ok(())
    }

    fn plan(
        &self,
        _base: BaseMember,
        _input: DeltaInput<SourceDelta>,
        context: &PlanContext,
        _budget: &mut WorkBudget,
    ) -> Result<PlanDecision<PlanContext>, IndexError> {
        Ok(PlanDecision::Incremental(context.clone()))
    }

    fn prepare<'a>(
        &'a self,
        plan: PlanContext,
        context: &'a mut AttemptContext<'_>,
    ) -> IndexFuture<'a, MemberBinding> {
        Box::pin(async move {
            let (payload, children, logical_commitment) =
                retired_layered_root(plan.instance.policy, plan.source);
            let artifact = context.put(&payload, &children)?;
            Ok(MemberBinding {
                instance: plan.instance.instance,
                format: plan.instance.format,
                artifact,
                input: plan.input,
                logical_commitment,
            })
        })
    }

    fn verify_and_seal<'a>(
        &'a self,
        candidate: MemberBinding,
        _context: &'a mut AttemptContext<'_>,
    ) -> IndexFuture<'a, MemberBinding> {
        Box::pin(async move { Ok(candidate) })
    }
}

fn engine_with<A>(
    objects: &Arc<FileObjectStore>,
    catalog: &Arc<SqliteCatalog>,
    scratch: &Arc<quanta_index_storage::FileScratchSpace>,
    adapter: A,
) -> TestResult<Kernel<SqliteCatalog>>
where
    A: IndexBuildPort<Delta = SourceDelta> + 'static,
{
    let mut registry = AdapterRegistry::new();
    let spec = text_spec(29);
    registry.register(
        AdapterDescriptor {
            kind: spec.kind,
            schema: spec.schema,
            format: spec.format,
        },
        adapter,
        |delta| Ok(delta.clone()),
    )?;
    Ok(Kernel::new(
        KernelServices::new(
            objects.clone(),
            objects.clone(),
            catalog.clone(),
            scratch.clone(),
            Duration::from_secs(30),
        )?,
        registry,
    ))
}

fn text_member(
    engine: &Kernel<SqliteCatalog>,
    objects: &FileObjectStore,
    view: ViewRef,
    instance: u8,
) -> TestResult<MemberBinding> {
    let ticket = engine.pin_view(ViewSelection::Exact(view), &budget())?;
    let mut key = b"member\0".to_vec();
    key.extend_from_slice(&[instance; 16]);
    let reference = objects
        .lookup_object(ticket.view().members, &key, &mut budget())?
        .ok_or("text member absent")?;
    Ok(serde_json::from_slice(
        &objects.get(reference, &mut budget())?.payload,
    )?)
}

fn search_one(
    engine: &Kernel<SqliteCatalog>,
    objects: &FileObjectStore,
    view: ViewRef,
    member: &MemberBinding,
) -> Result<Vec<OwnerKey>, IndexError> {
    let ticket = engine.pin_view(ViewSelection::Exact(view), &budget())?;
    let mut query = LqQuery::empty(LqSpan::synthetic(0));
    query.expr = LqExpr::Leaf(LqLeaf::Keyword("one".into()));
    let plan = LexicalPlanner::plan(&query)
        .map_err(|error| IndexError::InvalidInput(error.to_string()))?;
    let ranked = NativeTextIndex::new(recipe()).search_content(
        member,
        &plan,
        &QueryConstraintSetV1::unconstrained(),
        4,
        Bm25Policy { k1: 1.2, b: 0.75 },
        TextReadContext {
            ticket: ticket.record(),
            objects,
            maps: objects,
            budget: &mut budget(),
        },
    )?;
    let mut owners: Vec<OwnerKey> = ranked.hits.iter().map(|hit| hit.owner).collect();
    owners.sort_unstable();
    Ok(owners)
}

fn profile_request(
    engine: &Kernel<SqliteCatalog>,
    sequence: u64,
    expected_view: ViewRef,
    instance: u8,
) -> TestResult<UpdateRequest> {
    let profile = engine.stage_profile(
        engine.begin_input(operation(sequence))?,
        &IndexProfile {
            instances: vec![text_spec(instance)],
        },
        &mut budget(),
    )?;
    Ok(UpdateRequest {
        operation: operation(sequence),
        expected_view,
        kind: UpdateKind::Reconfigure { profile },
    })
}

#[test]
fn retired_text_root_is_rebuild_required_not_corrupt_and_rebuilds() -> TestResult {
    let root = tempfile::tempdir()?;
    let scratch = Arc::new(quanta_index_storage::FileScratchSpace::open_standalone(
        root.path().join("scratch"),
        16 * 1024 * 1024 * 1024,
    )?);
    let objects = Arc::new(FileObjectStore::open_with_scratch(
        root.path().join("native-objects"),
        scratch.clone(),
    )?);
    let catalog = Arc::new(SqliteCatalog::open(
        root.path(),
        Duration::from_millis(100),
    )?);
    let admitted = SourceTextAdmission::Utf8 {
        policy: Digest([31; 32]),
    };

    // The previous release publishes a view whose Text member is a QTXROOT3 root.
    let previous = engine_with(&objects, &catalog, &scratch, RetiredLayeredWriter)?;
    let genesis = previous.create_workspace(workspace(), 1, &mut budget())?;
    let configured = published(block_on(
        previous.submit(profile_request(&previous, 1, genesis, 29)?, &mut budget()),
    )?)?;
    let stage_text = |engine: &Kernel<SqliteCatalog>, key, path: &str, body: &str| {
        engine.stage_source_text(
            engine.begin_input(key)?,
            path,
            body.as_bytes(),
            admitted,
            &mut budget(),
        )
    };
    let a = stage_text(&previous, operation(2), "a.rs", "alpha one")?;
    let b = stage_text(&previous, operation(2), "b.rs", "beta one")?;
    let stale = published(block_on(
        previous.submit(
            native_text_cost_request(
                &previous,
                configured,
                source(&previous, configured)?,
                operation(2),
                1,
                [(1, a), (2, b)]
                    .into_iter()
                    .map(|(id, next)| OwnerMutation {
                        owner: owner(id),
                        expected: None,
                        next: Some(next),
                    })
                    .collect(),
            )?,
            &mut budget(),
        ),
    )?)?;
    drop(previous);

    // The current release opens the same Catalog and store.
    let engine = engine_with(&objects, &catalog, &scratch, NativeTextIndex::new(recipe()))?;
    let retired = text_member(&engine, &objects, stale, 29)?;
    assert!(
        objects
            .get(retired.artifact, &mut budget())?
            .payload
            .starts_with(b"QTXROOT3")
    );

    // Read time: typed rebuild-needed, never corruption.
    assert_eq!(
        search_one(&engine, &objects, stale, &retired),
        Err(IndexError::RebuildRequired)
    );
    // The store-level member scrub finds an intact object: nothing to quarantine.
    let scrubbed = engine.with_reproved_source_member_artifact(
        ViewSelection::Exact(stale),
        IndexInstanceId([29; 16]),
        &mut budget(),
        |artifact, intact, _budget| Ok((artifact, intact)),
    )?;
    assert_eq!(scrubbed, (retired.artifact, true));

    // Build time: a real DELTA over the stale member is refused as rebuild-needed
    // and publishes nothing.
    let c = stage_text(&engine, operation(3), "c.rs", "gamma one")?;
    let delta = native_text_cost_request(
        &engine,
        stale,
        source(&engine, stale)?,
        operation(3),
        2,
        vec![OwnerMutation {
            owner: owner(3),
            expected: None,
            next: Some(c),
        }],
    )?;
    assert_eq!(
        block_on(engine.submit(delta, &mut budget()))?,
        SubmitOutcome::Terminal(OperationStatus::Refused(Refusal::RebuildRequired))
    );
    let latest = engine.pin_view(
        ViewSelection::Latest {
            workspace: workspace(),
            epoch: 1,
        },
        &budget(),
    )?;
    assert_eq!(latest.view().identity, stale);
    latest.release()?;

    // Rebuild: replacing the Text instance builds a current member from the
    // selected Source snapshot, and the view is readable again.
    let rebuilt = published(block_on(
        engine.submit(profile_request(&engine, 4, stale, 30)?, &mut budget()),
    )?)?;
    let current = text_member(&engine, &objects, rebuilt, 30)?;
    assert!(
        objects
            .get(current.artifact, &mut budget())?
            .payload
            .starts_with(b"QTXROOT4")
    );
    assert_eq!(
        search_one(&engine, &objects, rebuilt, &current)?,
        vec![owner(1), owner(2)]
    );
    Ok(())
}
