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

fn assert_root_contribution(
    value: &quanta_index_core::index::MapValue,
    tag: u8,
    indexed_tokens: u64,
    record: ObjectRef,
) -> TestResult<ObjectRef> {
    let quanta_index_core::index::MapValue::Inline(inline) = value else {
        return Err("Block/Tree contribution still requires a wrapper object".into());
    };
    let mut expected = [0_u8; 9];
    expected[0] = tag;
    expected[1..].copy_from_slice(&indexed_tokens.to_be_bytes());
    assert_eq!(inline.data()?, expected.as_slice());
    let [source, inventory] = inline.children()? else {
        return Err("Block/Tree contribution has wrong physical children".into());
    };
    assert_eq!(*source, record);
    Ok(*inventory)
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

fn assert_shared_positions(
    objects: &FileObjectStore,
    ticket: &TicketRecord,
    binding: &MemberBinding,
    count: u8,
    ninth_positions: Option<&[u32]>,
) -> TestResult {
    let reader = NativeTextIndex::new(TextRecipe::new(Digest([31; 32])));
    for case in [CaseMode::Sensitive, CaseMode::Folded] {
        let mut hits = Vec::new();
        let scan = reader.visit_term(
            binding,
            case,
            "shared",
            TextReadContext {
                ticket,
                objects,
                maps: objects,
                budget: &mut budget(),
            },
            |hit| {
                hits.push((hit.owner, hit.tf, hit.positions.to_vec()));
                Ok(())
            },
        )?;
        assert_eq!(scan.df, u64::from(count));
        assert_eq!(hits.len(), usize::from(count));
        for id in 1..=count {
            let matching: Vec<_> = hits
                .iter()
                .filter(|(found, _, _)| *found == owner(id))
                .collect();
            assert_eq!(matching.len(), 1, "missing or duplicated owner {id}");
            let (_, tf, positions) = matching[0];
            let expected = if id == 9 {
                ninth_positions.unwrap_or(&[0, 1])
            } else {
                &[0, 1]
            };
            assert_eq!(*tf, u64::try_from(expected.len())?);
            assert_eq!(positions.as_slice(), expected);
        }
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
    let expanded = contribution_value(&engine, &objects, large)?;
    let block = assert_root_contribution(&expanded, 0x80, 5, second)?;
    let block_pin = engine.pin_view(ViewSelection::Exact(large), &budget())?;
    let stored_block = objects.get(block, &mut budget())?;
    // The initial a/b occurrence remains one old block; c/d/e are distinct
    // delta leaves. The two case surfaces select the same four child refs.
    let mut initial_block_wire = b"QITXEM02\0\x0a\0\x04".to_vec();
    for surface in [0_u8, 1_u8] {
        for (ordinal, letter) in (b'a'..=b'e').enumerate() {
            let slot = ordinal.saturating_sub(1);
            initial_block_wire.extend_from_slice(&[0, 2, surface, letter]);
            initial_block_wire.extend_from_slice(&u16::try_from(slot)?.to_be_bytes());
        }
    }
    assert_eq!(stored_block.payload, initial_block_wire);
    assert_eq!(stored_block.registration.bytes, 216);
    assert_eq!(stored_block.registration.children.len(), 4);
    assert_eq!(stored_block.registration.children.first(), edges.get(1));
    for reference in &stored_block.registration.children {
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
    let generic_contribution = contribution_value(&engine, &objects, generic)?;
    let _tree = assert_root_contribution(&generic_contribution, 0x81, 257, generic_source)?;
    assert_eq!(native_text_test_df(&engine, &objects, generic, "taaa")?, 1);
    assert_eq!(native_text_test_df(&engine, &objects, generic, "tajx")?, 0);
    let (returned, returned_source) =
        advance(&engine, generic, 6, Some(generic_source), b"a b c d e")?;
    let returned_contribution = contribution_value(&engine, &objects, returned)?;
    let returned_block =
        assert_root_contribution(&returned_contribution, 0x80, 5, returned_source)?;
    let returned_stored_block = objects.get(returned_block, &mut budget())?;
    // The intervening Tree removed a..e, so the return creates five distinct
    // leaves and a different dictionary despite identical ordered keys.
    let mut returned_block_wire = b"QITXEM02\0\x0a\0\x05".to_vec();
    for surface in [0_u8, 1_u8] {
        for (slot, letter) in (b'a'..=b'e').enumerate() {
            returned_block_wire.extend_from_slice(&[0, 2, surface, letter]);
            returned_block_wire.extend_from_slice(&u16::try_from(slot)?.to_be_bytes());
        }
    }
    assert_eq!(returned_stored_block.payload, returned_block_wire);
    assert_eq!(returned_stored_block.registration.bytes, 248);
    assert_eq!(returned_stored_block.registration.children.len(), 5);
    let returned_children = returned_stored_block.registration.children.clone();
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
    let generic_again_contribution = contribution_value(&engine, &objects, generic_again)?;
    let _generic_again_tree =
        assert_root_contribution(&generic_again_contribution, 0x81, 257, returned_wide_source)?;
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
    let returned_again_contribution = contribution_value(&engine, &objects, returned_again)?;
    let returned_again_block =
        assert_root_contribution(&returned_again_contribution, 0x80, 5, returned_again_source)?;
    assert_eq!(returned_again_block, returned_block);
    let returned_again_stored_block = objects.get(returned_again_block, &mut budget())?;
    assert_eq!(returned_again_stored_block.payload, returned_block_wire);
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
    assert_eq!(contribution_value(&reopened, &objects, large)?, expanded);
    assert_eq!(
        contribution_value(&reopened, &objects, generic)?,
        generic_contribution
    );
    assert_eq!(
        contribution_value(&reopened, &objects, returned)?,
        returned_contribution
    );
    assert_eq!(
        contribution_value(&reopened, &objects, generic_again)?,
        generic_again_contribution
    );
    assert_eq!(
        contribution_value(&reopened, &objects, returned_again)?,
        returned_again_contribution
    );
    assert_eq!(
        objects.get(block, &mut budget())?.payload,
        initial_block_wire
    );
    let reopened_returned_block = objects.get(returned_block, &mut budget())?;
    assert_eq!(reopened_returned_block.payload, returned_block_wire);
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
    native_text_inventory_root_survives_pin_then_reclaims(false)
}

#[test]
fn native_text_occurrence_inventory_stays_physical_through_exact_pin_then_reclaims() -> TestResult {
    native_text_inventory_root_survives_pin_then_reclaims(true)
}

fn native_text_inventory_root_survives_pin_then_reclaims(packed: bool) -> TestResult {
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
    let (prior_view, prior_source, sequence) = if packed {
        (configured, None, 2)
    } else {
        let (view, record) = advance(&engine, configured, 2, None, b"a")?;
        (view, Some(record), 3)
    };
    let (block_view, block_source) =
        advance(&engine, prior_view, sequence, prior_source, b"a b c d e")?;
    let block_pin = engine.pin_view(ViewSelection::Exact(block_view), &budget())?;
    let block_binding = member(&engine, &objects, block_view)?;
    let packed_terms = if packed {
        let artifact = objects.get(block_binding.artifact, &mut budget())?;
        let root: serde_json::Value = serde_json::from_slice(&artifact.payload)?;
        let terms: MapRoot = object_field(&root, "terms")?;
        let reference = terms.0.ok_or("missing terms root")?;
        let object = objects.get(reference, &mut budget())?;
        assert!(object.payload.starts_with(b"QIMAP005"));
        Some((reference, object.payload.clone()))
    } else {
        None
    };
    let contribution = contribution_value(&engine, &objects, block_view)?;
    let tag = if packed { 0x82 } else { 0x80 };
    let block = assert_root_contribution(&contribution, tag, 5, block_source)?;
    let stored_block = objects.get(block, &mut budget())?;
    let block_bytes = stored_block.payload.clone();
    let old_occurrence = if packed {
        assert!(stored_block.payload.starts_with(b"QITXOC01"));
        block
    } else {
        assert!(stored_block.payload.starts_with(b"QITXEM02"));
        *stored_block
            .registration
            .children
            .first()
            .ok_or("block has no occurrence child")?
    };
    let old_occurrence_object = objects.get(old_occurrence, &mut budget())?;
    assert!(old_occurrence_object.registration.children.is_empty());
    let old_occurrence_bytes = old_occurrence_object.payload.clone();
    drop(stored_block);
    let final_sequence = sequence + 1;
    let (latest, _small_source) = advance(
        &engine,
        block_view,
        final_sequence,
        Some(block_source),
        b"x",
    )?;
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
    for sequence in 1..=final_sequence {
        let key = operation(sequence);
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
    if let Some((reference, bytes)) = &packed_terms {
        assert_eq!(objects.get(*reference, &mut budget())?.payload, *bytes);
    }
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
    if let Some((reference, _)) = packed_terms {
        assert!(matches!(
            objects.get(reference, &mut budget()),
            Err(IndexError::CorruptObject(_))
        ));
    }
    assert!(matches!(
        std::fs::symlink_metadata(&physical),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound
    ));
    assert_eq!(native_text_test_df(&engine, &objects, latest, "x")?, 1);
    Ok(())
}

#[test]
fn posting_tree_witness_shrinks_to_small_and_reexpands_with_pinned_views() -> TestResult {
    use quanta_index_core::index::ports::{
        CatalogCustodyMaintenancePort, CatalogExactHistoryPort, CatalogReclaimPort,
        ManagedObjectReclaimPort,
    };

    let directory = tempfile::tempdir()?;
    let observer = Arc::new(quanta_index_storage::ObjectIoObservation::default());
    let ((engine, objects, catalog, spy), text_spec) =
        native_text_services(directory.path(), 64, 64, &observer)?;
    let genesis = engine.create_workspace(workspace(), 1, &mut budget())?;
    let configure = operation(1);
    let profile = engine.stage_profile(
        engine.begin_input(configure)?,
        &IndexProfile {
            instances: vec![text_spec],
        },
        &mut budget(),
    )?;
    let configured = published(block_on(engine.submit(
        UpdateRequest {
            operation: configure,
            expected_view: genesis,
            kind: UpdateKind::Reconfigure { profile },
        },
        &mut budget(),
    ))?)?;

    let add = operation(2);
    let inputs: Vec<_> = (1..=9_u8)
        .map(|id| SourceInput {
            path: format!("entry-{id}.rs"),
            bytes: b"shared shared".to_vec(),
            text: SourceTextAdmission::Utf8 {
                policy: Digest([31; 32]),
            },
        })
        .collect();
    let source_refs = engine.stage_sources(engine.begin_input(add)?, &inputs, &mut budget())?;
    assert_eq!(source_refs.len(), 9);
    let initial = published(block_on(
        engine.submit(
            native_text_cost_request(
                &engine,
                configured,
                source(&engine, configured)?,
                add,
                1,
                source_refs
                    .iter()
                    .copied()
                    .zip(1..=9_u8)
                    .map(|(reference, id)| OwnerMutation {
                        owner: owner(id),
                        expected: None,
                        next: Some(reference),
                    })
                    .collect(),
            )?,
            &mut budget(),
        ),
    )?)?;
    let initial_pin = engine.pin_view(ViewSelection::Exact(initial), &budget())?;
    let initial_binding = member(&engine, &objects, initial)?;
    assert_shared_positions(&objects, initial_pin.record(), &initial_binding, 9, None)?;
    let artifact = objects.get(initial_binding.artifact, &mut budget())?;
    let root: serde_json::Value = serde_json::from_slice(&artifact.payload)?;
    let terms: MapRoot = object_field(&root, "terms")?;
    let mut initial_dense_refs = Vec::new();
    for surface in [0_u8, 1_u8] {
        let mut key = vec![surface];
        key.extend_from_slice(b"shared");
        let value = objects
            .lookup(terms, &key, &mut budget())?
            .ok_or("nine-owner posting absent")?;
        let quanta_index_core::index::MapValue::Inline(value) = value else {
            return Err("nine-owner posting is not an inline dense root".into());
        };
        let mut expected_inline = [0_u8; 9];
        expected_inline[0] = 1;
        expected_inline[1..].copy_from_slice(&9_u64.to_be_bytes());
        assert_eq!(value.data()?, expected_inline.as_slice());
        let dense = value.single_child()?.ok_or("dense directory root absent")?;
        let directory = objects.get(dense, &mut budget())?;
        assert_eq!(directory.registration.reference, dense);
        assert_eq!(&directory.payload[..8], b"QITXDD01");
        assert_eq!(u32::from_be_bytes(directory.payload[8..12].try_into()?), 9);
        assert_eq!(u16::from_be_bytes(directory.payload[12..14].try_into()?), 1);
        assert_eq!(&directory.payload[14..16], &[0, 0]);
        assert_eq!(directory.payload.len(), 16 + 66);
        let [leaf] = directory.registration.children.as_slice() else {
            return Err("dense directory must own one physical page".into());
        };
        let leaf = *leaf;
        let page = objects.get(leaf, &mut budget())?;
        assert_eq!(page.registration.reference, leaf);
        assert!(page.registration.children.is_empty());
        assert_eq!(&page.payload[..8], b"QITXDP01");
        assert_eq!(u16::from_be_bytes(page.payload[8..10].try_into()?), 9);
        assert_eq!(&page.payload[10..12], &[0, 0]);
        assert_eq!(page.payload.len(), 12 + 9 * 64);
        for id in 1..=9_u8 {
            let ordinal = usize::from(id - 1);
            let start = 12 + ordinal * 64;
            let owner_key: Vec<_> = owner(id)
                .producer
                .0
                .into_iter()
                .chain(owner(id).stable_id.0)
                .collect();
            assert_eq!(&page.payload[start..start + 32], owner_key.as_slice());
            assert!(
                page.payload[start + 32..start + 64]
                    .iter()
                    .any(|byte| *byte != 0)
            );
            if ordinal > 0 {
                assert!(&page.payload[start - 64..start - 32] < &page.payload[start..start + 32]);
            }
        }
        assert_eq!(&directory.payload[16..48], &page.payload[12..44]);
        assert_eq!(
            &directory.payload[48..80],
            &page.payload[12 + 8 * 64..44 + 8 * 64]
        );
        assert_eq!(u16::from_be_bytes(directory.payload[80..82].try_into()?), 9);
        initial_dense_refs.push((dense, leaf));
    }

    let remove = operation(3);
    let small = published(block_on(engine.submit(
        native_text_cost_request(
            &engine,
            initial,
            source(&engine, initial)?,
            remove,
            2,
            vec![OwnerMutation {
                owner: owner(9),
                expected: Some(source_refs[8]),
                next: None,
            }],
        )?,
        &mut budget(),
    ))?)?;
    let small_pin = engine.pin_view(ViewSelection::Exact(small), &budget())?;
    let small_binding = member(&engine, &objects, small)?;
    assert_shared_positions(&objects, small_pin.record(), &small_binding, 8, None)?;
    let artifact = objects.get(small_binding.artifact, &mut budget())?;
    let root: serde_json::Value = serde_json::from_slice(&artifact.payload)?;
    let terms: MapRoot = object_field(&root, "terms")?;
    let value = objects
        .lookup(terms, b"\0shared", &mut budget())?
        .ok_or("eight-owner posting absent")?;
    let quanta_index_core::index::MapValue::Object(posting_ref) = value else {
        return Err("dense posting did not shrink to Small".into());
    };
    let posting = objects.get(posting_ref, &mut budget())?;
    let posting_json: serde_json::Value = serde_json::from_slice(&posting.payload)?;
    assert_eq!(
        posting_json.pointer("/root/Small/len"),
        Some(&serde_json::json!(8))
    );
    assert_eq!(posting.registration.children.len(), 8);

    let readd = operation(4);
    let replacement = engine.stage_sources(
        engine.begin_input(readd)?,
        &[SourceInput {
            path: "entry-9.rs".into(),
            bytes: b"lead shared shared".to_vec(),
            text: SourceTextAdmission::Utf8 {
                policy: Digest([31; 32]),
            },
        }],
        &mut budget(),
    )?[0];
    let latest = published(block_on(engine.submit(
        native_text_cost_request(
            &engine,
            small,
            source(&engine, small)?,
            readd,
            3,
            vec![OwnerMutation {
                owner: owner(9),
                expected: None,
                next: Some(replacement),
            }],
        )?,
        &mut budget(),
    ))?)?;
    let latest_pin = engine.pin_view(ViewSelection::Exact(latest), &budget())?;
    let latest_binding = member(&engine, &objects, latest)?;
    let latest_artifact = objects.get(latest_binding.artifact, &mut budget())?;
    let latest_root: serde_json::Value = serde_json::from_slice(&latest_artifact.payload)?;
    let latest_terms: MapRoot = object_field(&latest_root, "terms")?;
    let latest_value = objects
        .lookup(latest_terms, b"\0shared", &mut budget())?
        .ok_or("reexpanded posting absent")?;
    let quanta_index_core::index::MapValue::Inline(latest_inline) = latest_value else {
        return Err("reexpanded posting is not inline dense".into());
    };
    let mut expected_inline = [0_u8; 9];
    expected_inline[0] = 1;
    expected_inline[1..].copy_from_slice(&9_u64.to_be_bytes());
    assert_eq!(latest_inline.data()?, expected_inline.as_slice());
    let latest_dense = latest_inline
        .single_child()?
        .ok_or("reexpanded dense directory absent")?;
    let latest_directory = objects.get(latest_dense, &mut budget())?;
    assert_eq!(&latest_directory.payload[..8], b"QITXDD01");
    let [latest_leaf] = latest_directory.registration.children.as_slice() else {
        return Err("reexpanded dense directory must own one page".into());
    };
    let latest_page = objects.get(*latest_leaf, &mut budget())?;
    assert_eq!(&latest_page.payload[..8], b"QITXDP01");
    assert!(latest_page.registration.children.is_empty());
    assert_shared_positions(
        &objects,
        latest_pin.record(),
        &latest_binding,
        9,
        Some(&[1, 2]),
    )?;
    assert_shared_positions(&objects, initial_pin.record(), &initial_binding, 9, None)?;
    assert_shared_positions(&objects, small_pin.record(), &small_binding, 8, None)?;

    for key in [configure, add, remove, readd] {
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
    for _ in 0..16 {
        let page = catalog.reclaim_work_page(None, 8, &mut budget())?;
        if page.items.is_empty() {
            break;
        }
        for work in page.items {
            let _page = objects.reclaim_one(work, 32, &mut budget())?;
        }
    }
    for (dense, leaf) in initial_dense_refs {
        assert_eq!(
            objects.get(dense, &mut budget())?.registration.reference,
            dense
        );
        assert_eq!(
            objects.get(leaf, &mut budget())?.registration.reference,
            leaf
        );
    }
    assert_shared_positions(&objects, initial_pin.record(), &initial_binding, 9, None)?;
    assert_shared_positions(&objects, small_pin.record(), &small_binding, 8, None)?;
    assert_shared_positions(
        &objects,
        latest_pin.record(),
        &latest_binding,
        9,
        Some(&[1, 2]),
    )?;
    initial_pin.release()?;
    small_pin.release()?;
    latest_pin.release()?;

    drop(spy);
    drop(objects);
    drop(engine);
    drop(catalog);
    let ((reopened, objects, _catalog, _spy), _spec) =
        native_text_services(directory.path(), 64, 64, &observer)?;
    let reopened_pin = reopened.pin_view(ViewSelection::Exact(latest), &budget())?;
    let reopened_binding = member(&reopened, &objects, latest)?;
    assert_shared_positions(
        &objects,
        reopened_pin.record(),
        &reopened_binding,
        9,
        Some(&[1, 2]),
    )?;
    reopened_pin.release()?;
    Ok(())
}
