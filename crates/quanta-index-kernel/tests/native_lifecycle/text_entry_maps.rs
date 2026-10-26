//! Actual managed-object and reopen laws for bounded Text inventory/membership maps.
use super::*;
use quanta_index_core::index::{MapRoot, SourceInput, SourceTextAdmission};

fn object_field<T: serde::de::DeserializeOwned>(
    value: &serde_json::Value,
    key: &str,
) -> TestResult<T> {
    Ok(serde_json::from_value(
        value.get(key).ok_or("missing Text field")?.clone(),
    )?)
}

fn contribution(
    engine: &Kernel<SqliteCatalog>,
    objects: &FileObjectStore,
    view: ViewRef,
) -> TestResult<quanta_index_core::index::StoredObject> {
    let reference = contribution_value(engine, objects, view)?.object_ref()?;
    Ok(objects.get(reference, &mut budget())?)
}

fn contribution_value(
    engine: &Kernel<SqliteCatalog>,
    objects: &FileObjectStore,
    view: ViewRef,
) -> TestResult<quanta_index_core::index::MapValue> {
    let binding = member(engine, objects, view)?;
    let artifact = objects.get(binding.artifact, &mut budget())?;
    let root: serde_json::Value = serde_json::from_slice(&artifact.payload)?;
    let owners: MapRoot = object_field(&root, "owners")?;
    let selected = owner(1);
    let key: Vec<_> = selected
        .producer
        .0
        .iter()
        .chain(&selected.stable_id.0)
        .copied()
        .collect();
    Ok(objects
        .lookup(owners, &key, &mut budget())?
        .ok_or("missing contribution")?)
}

fn advance(
    engine: &Kernel<SqliteCatalog>,
    view: ViewRef,
    sequence: u64,
    prior: Option<ObjectRef>,
    body: &[u8],
) -> TestResult<(ViewRef, ObjectRef)> {
    let key = operation(sequence);
    let references = engine.stage_sources(
        engine.begin_input(key)?,
        &[SourceInput {
            path: "entry.rs".into(),
            bytes: body.to_vec(),
            text: SourceTextAdmission::Utf8 {
                policy: Digest([31; 32]),
            },
        }],
        &mut budget(),
    )?;
    let next = *references.first().ok_or("source was not staged")?;
    let request = native_text_cost_request(
        engine,
        view,
        source(engine, view)?,
        key,
        sequence.checked_sub(1).ok_or("event underflow")?,
        vec![OwnerMutation {
            owner: owner(1),
            expected: prior,
            next: Some(next),
        }],
    )?;
    Ok((
        published(block_on(engine.submit(request, &mut budget()))?)?,
        next,
    ))
}

#[test]
fn native_text_inline_children_tree_transition_and_old_pins_survive_reopen() -> TestResult {
    let directory = tempfile::tempdir()?;
    let observer = Arc::new(quanta_index_storage::ObjectIoObservation::default());
    let ((engine, objects, catalog, spy), text_spec) =
        native_text_services(directory.path(), 64, 64, &observer)?;
    let genesis = engine.create_workspace(workspace(), 1, &mut budget())?;
    let key = operation(1);
    let profile = engine.stage_profile(
        engine.begin_input(key)?,
        &IndexProfile {
            instances: vec![text_spec],
        },
        &mut budget(),
    )?;
    let configured = published(block_on(engine.submit(
        UpdateRequest {
            operation: key,
            expected_view: genesis,
            kind: UpdateKind::Reconfigure { profile },
        },
        &mut budget(),
    ))?)?;
    let (tiny, tiny_source) = advance(&engine, configured, 2, None, b"a b")?;
    let tiny_pin = engine.pin_view(ViewSelection::Exact(tiny), &budget())?;
    let tiny_value = contribution_value(&engine, &objects, tiny)?;
    let quanta_index_core::index::MapValue::Inline(tiny_data) = tiny_value else {
        return Err("two-token contribution still requires a wrapper object".into());
    };
    assert_eq!(
        tiny_data.data()?,
        b"\x04\0\0\0\0\0\0\0\x02\x02\0a\x02\0b\x02\x01a\x02\x01b"
    );
    let edges = tiny_data.children()?;
    assert_eq!(edges.len(), 5);
    assert_eq!(edges.first(), Some(&tiny_source));
    assert_eq!(edges.get(1), edges.get(3));
    assert_eq!(edges.get(2), edges.get(4));
    for (slot, reference) in edges.iter().skip(1).enumerate() {
        let occurrence = objects.get(*reference, &mut budget())?;
        assert!(occurrence.registration.children.is_empty());
        assert_eq!(
            serde_json::from_slice::<serde_json::Value>(&occurrence.payload)?,
            serde_json::json!({"positions": [slot % 2]})
        );
    }
    let (small, first) = advance(&engine, tiny, 3, Some(tiny_source), b"a b c d")?;
    let pin = engine.pin_view(ViewSelection::Exact(small), &budget())?;
    let initial = contribution(&engine, &objects, small)?;
    let value: serde_json::Value = serde_json::from_slice(&initial.payload)?;
    assert_eq!(
        value.pointer("/inventory/small/len"),
        Some(&serde_json::json!(8))
    );
    assert_eq!(initial.registration.children.len(), 9);
    assert_eq!(initial.registration.children.first(), Some(&first));
    for (slot, reference) in initial.registration.children.iter().skip(1).enumerate() {
        let occurrence = objects.get(*reference, &mut budget())?;
        assert!(occurrence.registration.children.is_empty());
        let gold = serde_json::json!({ "positions": [slot % 4] });
        assert_eq!(
            serde_json::from_slice::<serde_json::Value>(&occurrence.payload)?,
            gold
        );
    }
    let selected = member(&engine, &objects, small)?;
    let artifact = objects.get(selected.artifact, &mut budget())?;
    let root: serde_json::Value = serde_json::from_slice(&artifact.payload)?;
    let terms: MapRoot = object_field(&root, "terms")?;
    let posting = objects
        .lookup(terms, b"\0a", &mut budget())?
        .ok_or("singleton term absent")?;
    let quanta_index_core::index::MapValue::Inline(posting) = posting else {
        return Err("singleton posting still requires a wrapper object".into());
    };
    let gold_owner: Vec<_> = owner(1)
        .producer
        .0
        .into_iter()
        .chain(owner(1).stable_id.0)
        .collect();
    assert_eq!(posting.data()?, gold_owner);
    let occurrence = objects.get(
        posting
            .single_child()?
            .ok_or("singleton occurrence child absent")?,
        &mut budget(),
    )?;
    assert!(occurrence.registration.children.is_empty());
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(&occurrence.payload)?,
        serde_json::json!({"positions": [0]})
    );
    let cut = source(&engine, small)?;
    assert_eq!(
        objects.lookup(cut.paths, b"path\0entry.rs", &mut budget())?,
        Some(quanta_index_core::index::path_owner_map_value(owner(1))?)
    );
    let (large, second) = advance(&engine, small, 4, Some(first), b"a b c d e")?;
    let expanded = contribution(&engine, &objects, large)?;
    let expanded_value: serde_json::Value = serde_json::from_slice(&expanded.payload)?;
    let inventory = expanded_value.get("inventory").ok_or("inventory absent")?;
    let tree: MapRoot = object_field(inventory, "tree")?;
    assert!(tree.0.is_some());
    assert_eq!(inventory.pointer("/small/len"), Some(&serde_json::json!(0)));
    assert_eq!(expanded.registration.children.len(), 2);
    assert_eq!(expanded.registration.children.get(1), tree.0.as_ref());
    let (shrunk, _third) = advance(&engine, large, 5, Some(second), b"a b c d")?;
    assert_eq!(
        contribution(&engine, &objects, small)?.registration,
        initial.registration
    );
    assert_eq!(
        contribution(&engine, &objects, shrunk)?
            .registration
            .children
            .len(),
        9
    );
    for (view, term, expected) in [
        (small, "a", 1),
        (small, "e", 0),
        (large, "e", 1),
        (shrunk, "e", 0),
    ] {
        assert_eq!(
            native_text_test_df(&engine, &objects, view, term)?,
            expected
        );
    }
    assert_eq!(native_text_test_df(&engine, &objects, tiny, "a")?, 1);
    assert_eq!(contribution_value(&engine, &objects, tiny)?, tiny_value);
    tiny_pin.release()?;
    pin.release()?;
    drop(spy);
    drop(objects);
    drop(engine);
    drop(catalog);
    let ((reopened, objects, _catalog, _spy), _spec) =
        native_text_services(directory.path(), 64, 64, &observer)?;
    assert_eq!(contribution_value(&reopened, &objects, tiny)?, tiny_value);
    assert_eq!(native_text_test_df(&reopened, &objects, tiny, "a")?, 1);
    assert_eq!(
        contribution(&reopened, &objects, small)?.registration,
        initial.registration
    );
    assert_eq!(
        contribution(&reopened, &objects, large)?.registration,
        expanded.registration
    );
    assert_eq!(native_text_test_df(&reopened, &objects, shrunk, "a")?, 1);
    assert_eq!(native_text_test_df(&reopened, &objects, shrunk, "e")?, 0);
    Ok(())
}
