//! Actual managed-object and reopen laws for bounded Text inventory/membership maps.
use super::*;
use quanta_index_core::index::{MapRoot, SourceInput, SourceTextAdmission, TicketRecord};
use quanta_index_lexical::native::{CaseMode, NativeTextIndex, TextReadContext, TextRecipe};
use std::fmt::Write as _;

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

fn assert_term_positions(
    objects: &FileObjectStore,
    ticket: &TicketRecord,
    binding: &MemberBinding,
    term: &str,
    expected: &[u32],
) -> TestResult {
    let reader = NativeTextIndex::new(TextRecipe::new(Digest([31; 32])));
    for case in [CaseMode::Sensitive, CaseMode::Folded] {
        let mut observed = None;
        let scan = reader.visit_term(
            binding,
            case,
            term,
            TextReadContext {
                ticket,
                objects,
                maps: objects,
                budget: &mut budget(),
            },
            |hit| {
                if observed
                    .replace((hit.owner, hit.tf, hit.positions.to_vec()))
                    .is_some()
                {
                    return Err(IndexError::BackendViolation(
                        "single-owner term has multiple hits".into(),
                    ));
                }
                Ok(())
            },
        )?;
        assert_eq!(scan.df, 1);
        let (found_owner, tf, positions) = observed.ok_or("term hit absent")?;
        assert_eq!(found_owner, owner(1));
        assert_eq!(tf, u64::try_from(expected.len())?);
        assert_eq!(positions.as_slice(), expected);
    }
    Ok(())
}

#[test]
fn native_text_inline_small_block_generic_transitions_and_old_pins_survive_reopen() -> TestResult {
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
    for reference in edges.iter().skip(1) {
        let occurrence = objects.get(*reference, &mut budget())?;
        assert!(occurrence.registration.children.is_empty());
    }
    let tiny_binding = member(&engine, &objects, tiny)?;
    assert_term_positions(&objects, tiny_pin.record(), &tiny_binding, "a", &[0])?;
    assert_term_positions(&objects, tiny_pin.record(), &tiny_binding, "b", &[1])?;
    let (small, first) = advance(&engine, tiny, 3, Some(tiny_source), b"a b c d")?;
    let pin = engine.pin_view(ViewSelection::Exact(small), &budget())?;
    let initial = contribution(&engine, &objects, small)?;
    let value: serde_json::Value = serde_json::from_slice(&initial.payload)?;
    assert_eq!(
        value.pointer("/inventory/Small/len"),
        Some(&serde_json::json!(8))
    );
    assert_eq!(initial.registration.children.len(), 9);
    assert_eq!(initial.registration.children.first(), Some(&first));
    for reference in initial.registration.children.iter().skip(1) {
        let occurrence = objects.get(*reference, &mut budget())?;
        assert!(occurrence.registration.children.is_empty());
    }
    let selected = member(&engine, &objects, small)?;
    for (term, expected) in [("a", [0]), ("b", [1]), ("c", [2]), ("d", [3])] {
        assert_term_positions(&objects, pin.record(), &selected, term, &expected)?;
    }
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
        posting.single_child()?,
        initial.registration.children.get(1).copied()
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
    let block: ObjectRef = object_field(inventory, "Block")?;
    let block_pin = engine.pin_view(ViewSelection::Exact(large), &budget())?;
    assert_eq!(expanded.registration.children.len(), 2);
    assert_eq!(expanded.registration.children.as_slice(), &[second, block]);
    let stored_block = objects.get(block, &mut budget())?;
    let mut fixed_block_wire = b"QITXEM01\0\x0a\0\0".to_vec();
    for surface in [0_u8, 1_u8] {
        for letter in b'a'..=b'e' {
            fixed_block_wire.extend_from_slice(&[0, 2, surface, letter]);
        }
    }
    assert_eq!(stored_block.payload, fixed_block_wire);
    assert_eq!(stored_block.registration.bytes, 388);
    assert_eq!(stored_block.registration.children.len(), 10);
    for (slot, reference) in stored_block.registration.children.iter().enumerate() {
        assert_eq!(
            Some(reference),
            stored_block.registration.children.get(slot % 5)
        );
        let occurrence = objects.get(*reference, &mut budget())?;
        assert!(occurrence.registration.children.is_empty());
    }
    let expanded_binding = member(&engine, &objects, large)?;
    for (term, expected) in [("a", [0]), ("b", [1]), ("c", [2]), ("d", [3]), ("e", [4])] {
        assert_term_positions(
            &objects,
            block_pin.record(),
            &expanded_binding,
            term,
            &expected,
        )?;
    }
    // 257 terms have 514 raw/folded keys, exceeding the 512-row Block bound.
    let mut wide_body = String::new();
    for id in 0..257_u16 {
        let first = u8::try_from(id.checked_div(676).ok_or("term ordinal division")?)?;
        let middle = u8::try_from(id.checked_div(26).ok_or("term ordinal division")? % 26)?;
        let last = u8::try_from(id % 26)?;
        if !wide_body.is_empty() {
            wide_body.push(' ');
        }
        wide_body.push('t');
        for digit in [first, middle, last] {
            wide_body.push(char::from(
                b'a'.checked_add(digit).ok_or("wide term overflow")?,
            ));
        }
    }
    let (generic, generic_source) = advance(&engine, large, 5, Some(second), wide_body.as_bytes())?;
    let generic_pin = engine.pin_view(ViewSelection::Exact(generic), &budget())?;
    let generic_contribution = contribution(&engine, &objects, generic)?;
    let generic_value: serde_json::Value = serde_json::from_slice(&generic_contribution.payload)?;
    let generic_inventory = generic_value
        .get("inventory")
        .ok_or("generic inventory absent")?;
    let tree: ObjectRef = object_field(generic_inventory, "Tree")?;
    assert_eq!(
        generic_contribution.registration.children.as_slice(),
        &[generic_source, tree]
    );
    assert_eq!(native_text_test_df(&engine, &objects, generic, "taaa")?, 1);
    assert_eq!(native_text_test_df(&engine, &objects, generic, "tajx")?, 0);
    let (returned, returned_source) =
        advance(&engine, generic, 6, Some(generic_source), b"a b c d e")?;
    let returned_contribution = contribution(&engine, &objects, returned)?;
    let returned_value: serde_json::Value = serde_json::from_slice(&returned_contribution.payload)?;
    let returned_inventory = returned_value
        .get("inventory")
        .ok_or("returned inventory absent")?;
    let returned_block: ObjectRef = object_field(returned_inventory, "Block")?;
    let returned_stored_block = objects.get(returned_block, &mut budget())?;
    assert_eq!(returned_stored_block.payload, fixed_block_wire);
    assert_eq!(returned_stored_block.registration.children.len(), 10);
    let returned_children = returned_stored_block.registration.children.clone();
    assert_eq!(
        returned_contribution.registration.children.as_slice(),
        &[returned_source, returned_block]
    );
    let returned_pin = engine.pin_view(ViewSelection::Exact(returned), &budget())?;
    let returned_binding = member(&engine, &objects, returned)?;
    for (term, expected) in [("a", [0]), ("b", [1]), ("c", [2]), ("d", [3]), ("e", [4])] {
        assert_term_positions(
            &objects,
            returned_pin.record(),
            &returned_binding,
            term,
            &expected,
        )?;
    }
    returned_pin.release()?;

    // The first return may have different occurrence children from the first
    // Block: the intervening Tree replaced every old term. Prove identity
    // reuse only after repeating the same ordered key/reference mapping.
    let (generic_again, returned_wide_source) = advance(
        &engine,
        returned,
        7,
        Some(returned_source),
        wide_body.as_bytes(),
    )?;
    let generic_again_contribution = contribution(&engine, &objects, generic_again)?;
    let generic_again_value: serde_json::Value =
        serde_json::from_slice(&generic_again_contribution.payload)?;
    let generic_again_inventory = generic_again_value
        .get("inventory")
        .ok_or("second generic inventory absent")?;
    let _generic_again_tree: ObjectRef = object_field(generic_again_inventory, "Tree")?;
    assert_eq!(
        native_text_test_df(&engine, &objects, generic_again, "a")?,
        0
    );
    let (returned_again, returned_again_source) = advance(
        &engine,
        generic_again,
        8,
        Some(returned_wide_source),
        b"a b c d e",
    )?;
    let returned_again_contribution = contribution(&engine, &objects, returned_again)?;
    let returned_again_value: serde_json::Value =
        serde_json::from_slice(&returned_again_contribution.payload)?;
    let returned_again_inventory = returned_again_value
        .get("inventory")
        .ok_or("second returned inventory absent")?;
    let returned_again_block: ObjectRef = object_field(returned_again_inventory, "Block")?;
    assert_eq!(returned_again_block, returned_block);
    assert_eq!(
        returned_again_contribution.registration.children.as_slice(),
        &[returned_again_source, returned_again_block]
    );
    let returned_again_stored_block = objects.get(returned_again_block, &mut budget())?;
    assert_eq!(returned_again_stored_block.payload, fixed_block_wire);
    assert_eq!(
        returned_again_stored_block.registration.children,
        returned_children
    );
    let (shrunk, shrunk_source) = advance(
        &engine,
        returned_again,
        9,
        Some(returned_again_source),
        b"a b c d",
    )?;
    let (empty, _empty_source) = advance(&engine, shrunk, 10, Some(shrunk_source), b"")?;
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
        (generic, "a", 0),
        (returned, "e", 1),
        (generic_again, "a", 0),
        (returned_again, "e", 1),
        (shrunk, "e", 0),
        (empty, "a", 0),
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
    block_pin.release()?;
    generic_pin.release()?;
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
    assert_eq!(
        contribution(&reopened, &objects, generic)?.registration,
        generic_contribution.registration
    );
    assert_eq!(
        contribution(&reopened, &objects, returned)?.registration,
        returned_contribution.registration
    );
    assert_eq!(
        contribution(&reopened, &objects, generic_again)?.registration,
        generic_again_contribution.registration
    );
    assert_eq!(
        contribution(&reopened, &objects, returned_again)?.registration,
        returned_again_contribution.registration
    );
    assert_eq!(objects.get(block, &mut budget())?.payload, fixed_block_wire);
    let reopened_returned_block = objects.get(returned_block, &mut budget())?;
    assert_eq!(reopened_returned_block.payload, fixed_block_wire);
    assert_eq!(
        reopened_returned_block.registration.children,
        returned_children
    );
    let reopened_pin = reopened.pin_view(ViewSelection::Exact(large), &budget())?;
    let reopened_binding = member(&reopened, &objects, large)?;
    assert_term_positions(
        &objects,
        reopened_pin.record(),
        &reopened_binding,
        "a",
        &[0],
    )?;
    assert_term_positions(
        &objects,
        reopened_pin.record(),
        &reopened_binding,
        "e",
        &[4],
    )?;
    reopened_pin.release()?;
    assert_eq!(native_text_test_df(&reopened, &objects, shrunk, "a")?, 1);
    assert_eq!(native_text_test_df(&reopened, &objects, shrunk, "e")?, 0);
    assert_eq!(native_text_test_df(&reopened, &objects, empty, "a")?, 0);
    Ok(())
}

#[test]
fn native_text_block_stays_physical_through_exact_pin_then_reclaims() -> TestResult {
    use quanta_index_core::index::ports::{
        CatalogCustodyMaintenancePort, CatalogExactHistoryPort, CatalogReclaimPort,
        ManagedObjectReclaimPort,
    };

    let directory = tempfile::tempdir()?;
    let observer = Arc::new(quanta_index_storage::ObjectIoObservation::default());
    let ((engine, objects, catalog, _spy), text_spec) =
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
    let (block_view, block_source) = advance(&engine, configured, 2, None, b"a b c d e")?;
    let block_pin = engine.pin_view(ViewSelection::Exact(block_view), &budget())?;
    let block_binding = member(&engine, &objects, block_view)?;
    let contribution = contribution(&engine, &objects, block_view)?;
    let value: serde_json::Value = serde_json::from_slice(&contribution.payload)?;
    let block: ObjectRef =
        object_field(value.get("inventory").ok_or("inventory absent")?, "Block")?;
    let stored_block = objects.get(block, &mut budget())?;
    let block_bytes = stored_block.payload.clone();
    let old_occurrence = *stored_block
        .registration
        .children
        .first()
        .ok_or("block has no occurrence child")?;
    let old_occurrence_object = objects.get(old_occurrence, &mut budget())?;
    assert!(old_occurrence_object.registration.children.is_empty());
    let old_occurrence_bytes = old_occurrence_object.payload.clone();
    drop(stored_block);
    let (latest, _small_source) = advance(&engine, block_view, 3, Some(block_source), b"x")?;
    let physical = {
        let bytes = (block.0).0;
        let shard = format!(
            "{:02x}",
            bytes.first().copied().ok_or("object reference is empty")?
        );
        let mut name = String::with_capacity(64);
        for byte in bytes {
            write!(&mut name, "{byte:02x}")?;
        }
        directory
            .path()
            .join("native-objects/objects")
            .join(shard)
            .join(name)
    };
    assert!(
        physical.is_file(),
        "block object was not installed physically"
    );
    for key in [operation(1), operation(2), operation(3)] {
        loop {
            if !catalog
                .release_terminal_custody(key, 256, &mut budget())?
                .has_more
            {
                break;
            }
        }
    }
    let _floor =
        catalog.advance_exact_history_floor(workspace(), 1, latest.sequence, &mut budget())?;
    let retired = catalog.retire_exact_history_page(workspace(), 1, None, 16, &mut budget())?;
    assert!(retired.blocked >= 1);
    block_pin.validate()?;
    assert_eq!(objects.get(block, &mut budget())?.payload, block_bytes);
    assert_eq!(
        objects.get(old_occurrence, &mut budget())?.payload,
        old_occurrence_bytes
    );
    assert!(matches!(
        engine.pin_view(ViewSelection::Exact(block_view), &budget()),
        Err(IndexError::SnapshotUnavailable)
    ));
    assert_term_positions(&objects, block_pin.record(), &block_binding, "a", &[0])?;
    assert_term_positions(&objects, block_pin.record(), &block_binding, "e", &[4])?;
    assert!(physical.is_file());
    block_pin.release()?;
    let retired = catalog.retire_exact_history_page(workspace(), 1, None, 16, &mut budget())?;
    assert!(retired.retired >= 1);
    let mut reclaimed = false;
    for _ in 0..128 {
        let page = catalog.reclaim_work_page(None, 8, &mut budget())?;
        if page.items.is_empty() {
            break;
        }
        for work in page.items {
            let _page = objects.reclaim_one(work, 32, &mut budget())?;
        }
        if matches!(
            objects.get(block, &mut budget()),
            Err(IndexError::CorruptObject(_))
        ) {
            reclaimed = true;
            break;
        }
    }
    assert!(
        reclaimed,
        "block object retained after its last exact pin was released"
    );
    assert!(matches!(
        std::fs::symlink_metadata(&physical),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound
    ));
    assert_eq!(native_text_test_df(&engine, &objects, latest, "x")?, 1);
    Ok(())
}
