//! A frontier-only (no-op) Source DELTA does zero index I/O.
//!
//! This holds for every core-required `SourceLocal` member the product
//! registers: Literal, the file map, and Native Text in each layout. The view
//! stays logically identical.

use super::*;
use quanta_index_contract::{LqExpr, LqLeaf, LqQuery, LqSpan, QueryConstraintSetV1};
use quanta_index_core::index::SourceTextAdmission;
use quanta_index_lexical::native::{Bm25Policy, NativeTextIndex, TextReadContext, TextRecipe};
use quanta_index_lexical::planner::LexicalPlanner;
use std::collections::BTreeSet;

fn core_text_spec(recipe: TextRecipe) -> InstanceSpec {
    InstanceSpec {
        instance: IndexInstanceId([29; 16]),
        kind: IndexKind([28; 16]),
        policy: recipe.output_policy(),
        ..spec()
    }
}

fn core_fixture(
    root: &std::path::Path,
    recipe: TextRecipe,
) -> TestResult<(CostKernelFixture, Vec<InstanceSpec>)> {
    let io = Arc::new(quanta_index_storage::ObjectIoObservation::default());
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
        .with_io_observation(Arc::clone(&io)),
    );
    let spy = Arc::new(UpdateCostSpy {
        observe_reads: true,
        ..UpdateCostSpy::new_observed(Arc::clone(&objects), io)
    });
    let specs = vec![spec(), file_spec(), core_text_spec(recipe)];
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
    let text = core_text_spec(recipe);
    registry.register(
        AdapterDescriptor {
            kind: text.kind,
            schema: text.schema,
            format: text.format,
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
    Ok(((engine, objects, catalog, spy), specs))
}

fn member_of(
    engine: &Kernel<SqliteCatalog>,
    objects: &FileObjectStore,
    view: ViewRef,
    instance: IndexInstanceId,
) -> TestResult<MemberBinding> {
    let ticket = engine.pin_view(ViewSelection::Exact(view), &budget())?;
    let mut key = b"member\0".to_vec();
    key.extend_from_slice(&instance.0);
    let reference = objects
        .lookup_object(ticket.view().members, &key, &mut budget())?
        .ok_or("core member absent")?;
    Ok(serde_json::from_slice(
        &objects.get(reference, &mut budget())?.payload,
    )?)
}

/// All objects reachable from `roots`, including the roots themselves.
fn reachable(
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

/// Logical answers of all three members on one view.
#[derive(Debug, Eq, PartialEq)]
struct Answers {
    text: Vec<(OwnerKey, u64)>,
    literal: Vec<OwnerKey>,
    paths: Vec<String>,
}

fn answers(
    engine: &Kernel<SqliteCatalog>,
    objects: &FileObjectStore,
    recipe: TextRecipe,
    view: ViewRef,
) -> TestResult<Answers> {
    let ticket = engine.pin_view(ViewSelection::Exact(view), &budget())?;
    let text_member = member_of(engine, objects, view, core_text_spec(recipe).instance)?;
    let mut query = LqQuery::empty(LqSpan::synthetic(0));
    query.expr = LqExpr::Leaf(LqLeaf::Keyword("one".into()));
    let ranked = NativeTextIndex::new(recipe).search_content(
        &text_member,
        &LexicalPlanner::plan(&query)?,
        &QueryConstraintSetV1::unconstrained(),
        2,
        Bm25Policy { k1: 1.2, b: 0.75 },
        TextReadContext {
            ticket: ticket.record(),
            objects,
            maps: objects,
            budget: &mut budget(),
        },
    )?;
    let literal_member = member_of(engine, objects, view, spec().instance)?;
    let file_member = member_of(engine, objects, view, file_spec().instance)?;
    let mut work = budget();
    let page = query_paths(
        &FilePathQuery {
            member: &file_member,
            prefix: "",
            after: None,
            limit: 100,
        },
        &mut FileMapReadContext {
            ticket: ticket.record(),
            objects,
            maps: objects,
            budget: &mut work,
        },
    )?;
    Ok(Answers {
        text: ranked
            .hits
            .iter()
            .map(|hit| (hit.owner, hit.score.to_bits()))
            .collect(),
        literal: hits(engine, view, objects, &literal_member, b"one")?,
        paths: page
            .entries
            .iter()
            .map(|entry| entry.path.clone())
            .collect(),
    })
}

fn frontier_only_core_members_do_no_index_io(recipe: TextRecipe) -> TestResult {
    let directory = tempfile::tempdir()?;
    let ((engine, objects, _catalog, spy), specs) = core_fixture(directory.path(), recipe)?;
    let genesis = engine.create_workspace(workspace(), 1, &mut budget())?;
    let profile = engine.stage_profile(
        engine.begin_input(operation(1))?,
        &IndexProfile {
            instances: specs.clone(),
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
    let before = answers(&engine, &objects, recipe, first)?;
    // Independent gold: BM25 "one" ranks a.rs over b.rs; the literal matches
    // both; the file map lists every Source path, excluded bodies included.
    assert_eq!(
        before
            .text
            .iter()
            .map(|(owner, _)| *owner)
            .collect::<Vec<_>>(),
        vec![owner(1), owner(2)]
    );
    assert_eq!(before.literal, vec![owner(1), owner(2)]);
    assert_eq!(before.paths, ["a.rs", "b.rs", "binary.rs", "empty.rs"]);

    let base_source = source(&engine, first)?;
    let source_objects = reachable(
        &objects,
        [base_source.owners, base_source.paths, base_source.frontiers]
            .into_iter()
            .filter_map(|root| root.0),
    )?;
    let mut owned = Vec::new();
    for spec in &specs {
        let member = member_of(&engine, &objects, first, spec.instance)?;
        let member_objects: BTreeSet<ObjectRef> = reachable(&objects, [member.artifact])?
            .difference(&source_objects)
            .copied()
            .collect();
        assert!(member_objects.contains(&member.artifact));
        owned.push((spec.instance, member, member_objects));
    }

    let noop = native_text_cost_request(&engine, first, base_source, operation(3), 2, Vec::new())?;
    spy.arm(base_source.owners, BTreeSet::new())?;
    let outcome = block_on(engine.submit(noop, &mut budget()));
    let _observed = spy.disarm()?;
    let (reads, touched, installed) = {
        let state = spy
            .state
            .lock()
            .map_err(|error| std::io::Error::other(error.to_string()))?;
        (
            state
                .physical_reads
                .keys()
                .copied()
                .collect::<BTreeSet<_>>(),
            state.touched_maps.clone(),
            state.installed.clone(),
        )
    };
    let next = published(outcome?)?;
    let next_source = source(&engine, next)?;
    assert_eq!(next_source.owners, base_source.owners);
    assert_eq!(next_source.paths, base_source.paths);
    assert_ne!(next_source.frontiers, base_source.frontiers);

    for (instance, base, member_objects) in &owned {
        let next_member = member_of(&engine, &objects, next, *instance)?;
        assert_eq!(
            (next_member.artifact, next_member.logical_commitment),
            (base.artifact, base.logical_commitment),
            "{instance:?} did not keep its artifact"
        );
        assert_eq!(next_member.input.root, base.input.root);
        assert_ne!(next_member.input.context, base.input.context);
        for (label, touched_set) in [
            ("read", &reads),
            ("map access", &touched),
            ("install", &installed),
        ] {
            let overlap: Vec<_> = member_objects.intersection(touched_set).collect();
            assert!(
                overlap.is_empty(),
                "{instance:?} member objects had a {label}: {overlap:?}"
            );
        }
    }

    assert_eq!(answers(&engine, &objects, recipe, next)?, before);
    assert_eq!(answers(&engine, &objects, recipe, first)?, before);
    Ok(())
}

#[test]
fn frontier_only_delta_does_no_index_io_with_tree_text() -> TestResult {
    frontier_only_core_members_do_no_index_io(TextRecipe::new(Digest([31; 32])))
}

#[test]
fn frontier_only_delta_does_no_index_io_with_segmented_full_text() -> TestResult {
    frontier_only_core_members_do_no_index_io(TextRecipe::segmented_full(Digest([31; 32])))
}

#[test]
fn frontier_only_delta_does_no_index_io_with_layered_text() -> TestResult {
    frontier_only_core_members_do_no_index_io(TextRecipe::segmented_layered(Digest([31; 32])))
}
