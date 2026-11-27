//! A frontier-only (no-op) Source DELTA over a Layered Native Text member.
//! The view must stay logically identical and the Text segment must perform no
//! physical root, partition, segment or posting work.

use super::*;
use quanta_index_contract::{LqExpr, LqLeaf, LqQuery, LqSpan, QueryConstraintSetV1};
use quanta_index_core::index::SourceTextAdmission;
use quanta_index_lexical::native::{Bm25Policy, NativeTextIndex, TextReadContext, TextRecipe};
use quanta_index_lexical::planner::LexicalPlanner;
use std::collections::BTreeSet;

const RANKING: Bm25Policy = Bm25Policy { k1: 1.2, b: 0.75 };

fn layered_fixture(
    root: &std::path::Path,
    io: &Arc<quanta_index_storage::ObjectIoObservation>,
) -> TestResult<(CostKernelFixture, InstanceSpec, TextRecipe)> {
    let scratch = Arc::new(quanta_index_storage::FileScratchSpace::open_standalone(
        root.join("scratch"),
        16 * 1024 * 1024 * 1024,
    )?);
    let catalog = Arc::new(SqliteCatalog::open(root, Duration::from_millis(100))?);
    let objects = Arc::new(
        FileObjectStore::open_managed_with_scratch(
            root.join("native-objects"),
            scratch.clone(),
            catalog.clone(),
        )?
        .with_io_observation(Arc::clone(io)),
    );
    let spy = Arc::new(UpdateCostSpy {
        observe_reads: true,
        ..UpdateCostSpy::new_observed(Arc::clone(&objects), Arc::clone(io))
    });
    let recipe = TextRecipe::segmented_layered(Digest([31; 32]));
    let text_spec = InstanceSpec {
        kind: IndexKind([28; 16]),
        policy: recipe.output_policy(),
        ..spec()
    };
    let mut registry = AdapterRegistry::new();
    registry.register(
        AdapterDescriptor {
            kind: text_spec.kind,
            schema: text_spec.schema,
            format: text_spec.format,
        },
        NativeTextIndex::new(recipe),
        |delta| Ok(delta.clone()),
    )?;
    let engine = Kernel::new(
        KernelServices::new(
            spy.clone(),
            spy.clone(),
            catalog.clone(),
            scratch,
            Duration::from_secs(30),
        )?,
        registry,
    );
    Ok(((engine, objects, catalog, spy), text_spec, recipe))
}

/// All objects reachable from `roots`, including the roots themselves.
fn closure(
    objects: &FileObjectStore,
    roots: impl IntoIterator<Item = ObjectRef>,
) -> TestResult<BTreeSet<ObjectRef>> {
    let mut seen = BTreeSet::new();
    let mut pending: Vec<ObjectRef> = roots.into_iter().collect();
    while let Some(reference) = pending.pop() {
        if seen.insert(reference) {
            let object = objects.get(reference, &mut budget())?;
            pending.extend(object.registration.children.iter().copied());
        }
    }
    Ok(seen)
}

/// `(owner, durable source, score bits)` for one ranked content query.
fn ranked_one(
    engine: &Kernel<SqliteCatalog>,
    objects: &FileObjectStore,
    recipe: TextRecipe,
    view: ViewRef,
) -> TestResult<Vec<(OwnerKey, ObjectRef, u64)>> {
    let mut query = LqQuery::empty(LqSpan::synthetic(0));
    query.expr = LqExpr::Leaf(LqLeaf::Keyword("one".into()));
    let plan = LexicalPlanner::plan(&query)?;
    let ticket = engine.pin_view(ViewSelection::Exact(view), &budget())?;
    let binding = member(engine, objects, view)?;
    let ranked = NativeTextIndex::new(recipe).search_content(
        &binding,
        &plan,
        &QueryConstraintSetV1::unconstrained(),
        2,
        RANKING,
        TextReadContext {
            ticket: ticket.record(),
            objects,
            maps: objects,
            budget: &mut budget(),
        },
    )?;
    assert_eq!(ranked.view, view);
    assert_eq!(ranked.artifact, binding.artifact);
    assert_eq!(ranked.matched, 2);
    ranked
        .hits
        .iter()
        .map(|hit| {
            Ok((
                hit.owner,
                durable_text_source(hit.source)?,
                hit.score.to_bits(),
            ))
        })
        .collect()
}

#[test]
fn layered_text_noop_delta_reuses_root_without_text_segment_io() -> TestResult {
    let directory = tempfile::tempdir()?;
    let io = Arc::new(quanta_index_storage::ObjectIoObservation::default());
    let ((engine, objects, _catalog, spy), text_spec, recipe) =
        layered_fixture(directory.path(), &io)?;
    let genesis = engine.create_workspace(workspace(), 1, &mut budget())?;
    let profile = engine.stage_profile(
        engine.begin_input(operation(1))?,
        &IndexProfile {
            instances: vec![text_spec],
        },
        &mut budget(),
    )?;
    let configured = published(block_on(engine.submit(
        UpdateRequest {
            operation: operation(1),
            expected_view: genesis,
            kind: UpdateKind::Reconfigure { profile },
        },
        &mut budget(),
    ))?)?;
    let admitted = SourceTextAdmission::Utf8 {
        policy: Digest([31; 32]),
    };
    let stage_text = |path: &str, bytes: &[u8], text| -> TestResult<ObjectRef> {
        Ok(engine.stage_source_text(
            engine.begin_input(operation(2))?,
            path,
            bytes,
            text,
            &mut budget(),
        )?)
    };
    let a = stage_text("a.rs", "Café CAFÉ one one".as_bytes(), admitted)?;
    let b = stage_text("b.rs", "cafe\u{301} one".as_bytes(), admitted)?;
    let empty = stage_text("empty.rs", b"", admitted)?;
    let excluded = stage_text("binary.rs", &[0xff], SourceTextAdmission::Excluded)?;
    let first = published(block_on(
        engine.submit(
            native_text_cost_request(
                &engine,
                configured,
                source(&engine, configured)?,
                operation(2),
                1,
                [(1, a), (2, b), (3, empty), (4, excluded)]
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
    // Fixed independent BM25 gold, shared with the Tree layout lifecycle test:
    // N=3 (binary excluded), df=2, avgdl=2, (tf,dl)=(2,4),(1,2).
    let gold = vec![
        (owner(1), a, 0.504_394_138_702_740_6_f64),
        (owner(2), b, 0.470_003_629_245_735_5_f64),
    ];
    let before = ranked_one(&engine, &objects, recipe, first)?;
    assert_eq!(before.len(), gold.len());
    for ((owner, source, bits), (gold_owner, gold_source, gold_score)) in before.iter().zip(&gold) {
        assert_eq!((*owner, *source), (*gold_owner, *gold_source));
        assert!(score_within_16_ulps(f64::from_bits(*bits), *gold_score));
    }

    let base_source = source(&engine, first)?;
    let base_member = member(&engine, &objects, first)?;
    let source_objects = closure(
        &objects,
        [base_source.owners, base_source.paths, base_source.frontiers]
            .into_iter()
            .filter_map(|root| root.0),
    )?;
    let text_objects: BTreeSet<ObjectRef> = closure(&objects, [base_member.artifact])?
        .difference(&source_objects)
        .copied()
        .collect();
    assert!(text_objects.contains(&base_member.artifact));
    assert!(
        text_objects.len() > 1,
        "the Layered root must own at least one segment child"
    );

    let noop = native_text_cost_request(&engine, first, base_source, operation(3), 2, Vec::new())?;
    spy.arm(base_source.owners, BTreeSet::new())?;
    let outcome = block_on(engine.submit(noop, &mut budget()));
    let observed = spy.disarm()?;
    let (text_reads, text_wire_installs) = {
        let state = spy
            .state
            .lock()
            .map_err(|error| std::io::Error::other(error.to_string()))?;
        let reads: Vec<(ObjectRef, u64)> = state
            .physical_reads
            .iter()
            .filter(|(reference, _)| text_objects.contains(reference))
            .map(|(reference, count)| (*reference, *count))
            .collect();
        (reads, state.text_wire_installs)
    };
    let next = published(outcome?)?;

    // Logical identity: same owners and paths, an advanced frontier, and the
    // exact same Text root and logical commitment under the new input.
    let next_source = source(&engine, next)?;
    assert_eq!(next_source.owners, base_source.owners);
    assert_eq!(next_source.paths, base_source.paths);
    assert_ne!(next_source.frontiers, base_source.frontiers);
    let next_member = member(&engine, &objects, next)?;
    assert_eq!(next_member.artifact, base_member.artifact);
    assert_eq!(
        next_member.logical_commitment,
        base_member.logical_commitment
    );
    assert_eq!(next_member.input.root, base_member.input.root);
    assert_ne!(next_member.input.context, base_member.input.context);

    // Physical: no Text root/partition/segment/posting read, and no Text wire
    // object handed to the installer by the no-op publication.
    assert_eq!(text_reads, Vec::new(), "no-op DELTA read Text objects");
    assert_eq!(text_wire_installs, 0, "no-op DELTA wrote Text objects");
    assert!(
        observed.object_get_calls > 0,
        "the read observer must remain armed for Source/Catalog work"
    );

    // Search results and scores are bit-identical on the new view, and the
    // pinned prior view remains readable with the same answer.
    assert_eq!(ranked_one(&engine, &objects, recipe, next)?, before);
    assert_eq!(ranked_one(&engine, &objects, recipe, first)?, before);
    Ok(())
}
