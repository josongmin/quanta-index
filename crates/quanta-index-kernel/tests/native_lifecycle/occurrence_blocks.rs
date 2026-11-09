//! Real managed-store lifecycle for the new Text occurrence block policy.

use super::*;
use quanta_index_contract::{LqCase, LqExpr, LqLeaf, LqQuery, LqSpan};
use quanta_index_core::index::{
    MapRoot, MapValue, SourceTextAdmission, TicketRecord,
    ports::{
        CatalogCustodyMaintenancePort, CatalogExactHistoryPort, CatalogReclaimPort,
        ManagedObjectReclaimPort,
    },
};
use quanta_index_lexical::{
    native::{
        Bm25Policy, CaseMode, NativeTextIndex, NativeTextRanking, TextReadContext, TextRecipe,
    },
    planner::LexicalPlanner,
};

fn member_by_instance(
    objects: &FileObjectStore,
    ticket: &TicketRecord,
    instance: IndexInstanceId,
) -> TestResult<MemberBinding> {
    let mut key = b"member\0".to_vec();
    key.extend_from_slice(&instance.0);
    let reference = objects
        .lookup_object(ticket.view.members, &key, &mut budget())?
        .ok_or("profile member absent")?;
    Ok(serde_json::from_slice(
        &objects.get(reference, &mut budget())?.payload,
    )?)
}

fn contribution_children(
    objects: &FileObjectStore,
    binding: &MemberBinding,
) -> TestResult<Vec<ObjectRef>> {
    let artifact = objects.get(binding.artifact, &mut budget())?;
    let root: serde_json::Value = serde_json::from_slice(&artifact.payload)?;
    let owners: MapRoot =
        serde_json::from_value(root.get("owners").ok_or("Text root has no owners")?.clone())?;
    let key: Vec<_> = owner(1)
        .producer
        .0
        .iter()
        .chain(&owner(1).stable_id.0)
        .copied()
        .collect();
    let contribution = objects
        .lookup(owners, &key, &mut budget())?
        .ok_or("Text owner contribution absent")?;
    match contribution {
        MapValue::Inline(inline) => Ok(inline.children()?.to_vec()),
        MapValue::Object(reference) => {
            let stored = objects.get(reference, &mut budget())?;
            Ok(stored.registration.children.clone())
        }
    }
}

// A cold exact packed inventory owns the occurrence directly. The first
// changed selector forces the warm fallback to a separate inventory block.
fn inventory_block_refs(
    objects: &FileObjectStore,
    binding: &MemberBinding,
    terms: &[String],
    last_term_changed: bool,
    record: ObjectRef,
) -> TestResult<(ObjectRef, Vec<ObjectRef>)> {
    let artifact = objects.get(binding.artifact, &mut budget())?;
    let root: serde_json::Value = serde_json::from_slice(&artifact.payload)?;
    let owners: MapRoot =
        serde_json::from_value(root.get("owners").ok_or("Text root has no owners")?.clone())?;
    let key: Vec<_> = owner(1)
        .producer
        .0
        .iter()
        .chain(&owner(1).stable_id.0)
        .copied()
        .collect();
    let contribution = objects
        .lookup(owners, &key, &mut budget())?
        .ok_or("Text owner contribution absent")?;
    let MapValue::Inline(inline) = contribution else {
        return Err("Block contribution still requires a wrapper object".into());
    };
    let indexed_tokens = u64::try_from(terms.len())? + u64::from(last_term_changed);
    let mut expected = [0_u8; 9];
    expected[0] = if last_term_changed { 0x80 } else { 0x82 };
    expected[1..].copy_from_slice(&indexed_tokens.to_be_bytes());
    assert_eq!(inline.data()?, expected.as_slice());
    let [source, block_ref] = inline.children()? else {
        return Err("Block contribution has wrong physical children".into());
    };
    assert_eq!(*source, record);
    let block_ref = *block_ref;
    let block = objects.get(block_ref, &mut budget())?;
    let row_count = u16::try_from(terms.len().checked_mul(2).ok_or("row count overflow")?)?;
    if !last_term_changed {
        let vector_count = u16::try_from(terms.len())?;
        let mut golden = b"QITXOC01".to_vec();
        golden.extend_from_slice(&row_count.to_be_bytes());
        golden.extend_from_slice(&vector_count.to_be_bytes());
        golden.extend_from_slice(&[0; 4]);
        for case in [0_u8, 1_u8] {
            for (index, term) in terms.iter().enumerate() {
                let key_length = u16::try_from(term.len().checked_add(1).ok_or("key overflow")?)?;
                golden.extend_from_slice(&key_length.to_be_bytes());
                golden.push(case);
                golden.extend_from_slice(term.as_bytes());
                golden.extend_from_slice(&u16::try_from(index)?.to_be_bytes());
            }
        }
        for index in 0..terms.len() {
            golden.extend_from_slice(&1_u16.to_be_bytes());
            golden.extend_from_slice(&u32::try_from(index)?.to_be_bytes());
        }
        assert!(block.registration.children.is_empty());
        assert_eq!(block.payload, golden);
        return Ok((block_ref, vec![block_ref; usize::from(row_count)]));
    }
    let mut golden = b"QITXEM02".to_vec();
    golden.extend_from_slice(&row_count.to_be_bytes());
    let unique_count = 2_u16;
    golden.extend_from_slice(&unique_count.to_be_bytes());
    assert_eq!(block.registration.children.len(), usize::from(unique_count));
    for (index, child) in block.registration.children.iter().enumerate() {
        assert!(
            block.registration.children[..index]
                .iter()
                .all(|prior| prior != child)
        );
    }
    let mut selected = Vec::with_capacity(usize::from(row_count));
    for case in [0_u8, 1_u8] {
        for (index, term) in terms.iter().enumerate() {
            let key_length = u16::try_from(term.len().checked_add(1).ok_or("key overflow")?)?;
            golden.extend_from_slice(&key_length.to_be_bytes());
            golden.push(case);
            golden.extend_from_slice(term.as_bytes());
            let slot = u16::from(index + 1 == terms.len());
            golden.extend_from_slice(&slot.to_be_bytes());
            selected.push(
                *block
                    .registration
                    .children
                    .get(usize::from(slot))
                    .ok_or("inventory child slot out of bounds")?,
            );
        }
    }
    assert_eq!(block.payload, golden);
    Ok((block_ref, selected))
}

fn occurrence_children(
    objects: &FileObjectStore,
    binding: &MemberBinding,
) -> TestResult<[ObjectRef; 4]> {
    let children = contribution_children(objects, binding)?;
    let [
        _,
        sensitive_alpha,
        sensitive_beta,
        folded_alpha,
        folded_beta,
    ] = children.as_slice()
    else {
        return Err("four-term contribution has wrong child count".into());
    };
    Ok([
        *sensitive_alpha,
        *sensitive_beta,
        *folded_alpha,
        *folded_beta,
    ])
}

fn term_positions(
    reader: &NativeTextIndex,
    binding: &MemberBinding,
    ticket: &TicketRecord,
    objects: &FileObjectStore,
    term: &str,
) -> TestResult<(u64, Vec<u32>)> {
    term_positions_case(reader, binding, ticket, objects, CaseMode::Sensitive, term)
}

fn term_positions_case(
    reader: &NativeTextIndex,
    binding: &MemberBinding,
    ticket: &TicketRecord,
    objects: &FileObjectStore,
    case: CaseMode,
    term: &str,
) -> TestResult<(u64, Vec<u32>)> {
    let mut hits = Vec::new();
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
            hits.push((hit.owner, hit.tf, hit.positions.to_vec()));
            Ok(())
        },
    )?;
    assert_eq!(scan.df, 1);
    let [(found_owner, tf, positions)] = hits.as_slice() else {
        return Err("expected one complete Text term hit".into());
    };
    assert_eq!(*found_owner, owner(1));
    Ok((*tf, positions.clone()))
}

fn ranked_leaf(
    reader: &NativeTextIndex,
    binding: &MemberBinding,
    ticket: &TicketRecord,
    objects: &FileObjectStore,
    leaf: LqLeaf,
) -> TestResult<NativeTextRanking> {
    let mut query = LqQuery::empty(LqSpan::synthetic(0));
    query.expr = LqExpr::Leaf(leaf);
    query.options.case = Some(LqCase::Sensitive);
    let plan = LexicalPlanner::plan(&query)?;
    Ok(reader.search_content(
        binding,
        &plan,
        &quanta_index_contract::QueryConstraintSetV1::unconstrained(),
        1,
        Bm25Policy { k1: 1.2, b: 0.75 },
        TextReadContext {
            ticket,
            objects,
            maps: objects,
            budget: &mut budget(),
        },
    )?)
}

fn phrase_source(
    reader: &NativeTextIndex,
    binding: &MemberBinding,
    ticket: &TicketRecord,
    objects: &FileObjectStore,
    phrase: &str,
) -> TestResult<ObjectRef> {
    let ranking = ranked_leaf(
        reader,
        binding,
        ticket,
        objects,
        LqLeaf::Phrase(phrase.into()),
    )?;
    assert_eq!(ranking.matched, 1);
    let [hit] = ranking.hits.as_slice() else {
        return Err("phrase did not select one owner".into());
    };
    assert_eq!(hit.owner, owner(1));
    durable_text_source(hit.source)
}

#[test]
fn source_before_profile_bootstrap_block_delta_leaf_and_reopen_old_selection() -> TestResult {
    occurrence_block_lifecycle(true)
}

#[test]
fn profile_before_source_first_patch_block_delta_leaf_and_reopen_old_selection() -> TestResult {
    occurrence_block_lifecycle(false)
}

#[test]
fn first_patch_one_block_over_256_selectors_preserves_delta_and_reopen() -> TestResult {
    let directory = tempfile::tempdir()?;
    let observer = Arc::new(quanta_index_storage::ObjectIoObservation::default());
    let ((engine, objects, catalog, spy), text_spec) =
        native_text_services(directory.path(), 64, 64, &observer)?;
    let genesis = engine.create_workspace(workspace(), 1, &mut budget())?;
    let profile = engine.stage_profile(
        engine.begin_input(operation(1))?,
        &IndexProfile {
            instances: vec![text_spec.clone()],
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

    // ASCII letters remain one token. Sensitive and folded keys give 260
    // distinct canonical selectors for one owner and one position each.
    let terms: Vec<_> = (0..130_u8)
        .map(|ordinal| {
            let high = char::from(b'a' + ordinal / 26);
            let low = char::from(b'a' + ordinal % 26);
            format!("q{high}{low}")
        })
        .collect();
    let first = terms.first().ok_or("first corpus term absent")?;
    let last = terms.last().ok_or("last corpus term absent")?;
    let initial_body = terms.join(" ");
    let initial_record = engine.stage_source_text(
        engine.begin_input(operation(2))?,
        "entry.rs",
        initial_body.as_bytes(),
        SourceTextAdmission::Utf8 {
            policy: Digest([31; 32]),
        },
        &mut budget(),
    )?;
    let old = published(block_on(engine.submit(
        native_text_cost_request(
            &engine,
            configured,
            source(&engine, configured)?,
            operation(2),
            1,
            vec![OwnerMutation {
                owner: owner(1),
                expected: None,
                next: Some(initial_record),
            }],
        )?,
        &mut budget(),
    ))?)?;
    let old_pin = engine.pin_view(ViewSelection::Exact(old), &budget())?;
    let old_member = member(&engine, &objects, old)?;
    let (_old_inventory, old_refs) =
        inventory_block_refs(&objects, &old_member, &terms, false, initial_record)?;
    assert_eq!(old_refs.len(), 260);
    let block_ref = *old_refs.first().ok_or("first occurrence child absent")?;
    assert!(old_refs.iter().all(|reference| *reference == block_ref));
    let block = objects.get(block_ref, &mut budget())?;
    assert!(block.registration.children.is_empty());
    assert!(block.payload.starts_with(b"QITXOC01"));
    let reader = NativeTextIndex::new(TextRecipe::new(Digest([31; 32])));
    assert_eq!(
        term_positions_case(
            &reader,
            &old_member,
            old_pin.record(),
            &objects,
            CaseMode::Sensitive,
            first,
        )?,
        (1, vec![0])
    );
    assert_eq!(
        term_positions_case(
            &reader,
            &old_member,
            old_pin.record(),
            &objects,
            CaseMode::Folded,
            last,
        )?,
        (1, vec![129])
    );

    let changed_body = format!("{initial_body} {last}");
    let changed_record = engine.stage_source_text(
        engine.begin_input(operation(3))?,
        "entry.rs",
        changed_body.as_bytes(),
        SourceTextAdmission::Utf8 {
            policy: Digest([31; 32]),
        },
        &mut budget(),
    )?;
    let current = published(block_on(engine.submit(
        native_text_cost_request(
            &engine,
            old,
            source(&engine, old)?,
            operation(3),
            2,
            vec![OwnerMutation {
                owner: owner(1),
                expected: Some(initial_record),
                next: Some(changed_record),
            }],
        )?,
        &mut budget(),
    ))?)?;
    let current_pin = engine.pin_view(ViewSelection::Exact(current), &budget())?;
    let current_member = member(&engine, &objects, current)?;
    let (_current_inventory, current_refs) =
        inventory_block_refs(&objects, &current_member, &terms, true, changed_record)?;
    assert_eq!(current_refs.len(), 260);
    assert_eq!(current_refs.first(), Some(&block_ref));
    assert_ne!(current_refs.last(), Some(&block_ref));
    assert_eq!(
        term_positions(
            &reader,
            &current_member,
            current_pin.record(),
            &objects,
            first
        )?,
        (1, vec![0])
    );
    assert_eq!(
        term_positions_case(
            &reader,
            &current_member,
            current_pin.record(),
            &objects,
            CaseMode::Folded,
            last,
        )?,
        (2, vec![129, 130])
    );
    old_pin.validate()?;
    assert_eq!(
        term_positions(&reader, &old_member, old_pin.record(), &objects, last)?,
        (1, vec![129])
    );
    old_pin.release()?;
    current_pin.release()?;
    drop(spy);
    drop(objects);
    drop(engine);
    drop(catalog);

    let ((reopened, objects, reopened_catalog, _spy), _spec) =
        native_text_services(directory.path(), 64, 64, &observer)?;
    let old_pin = reopened.pin_view(ViewSelection::Exact(old), &budget())?;
    let current_pin = reopened.pin_view(ViewSelection::Exact(current), &budget())?;
    let old_member = member(&reopened, &objects, old)?;
    let current_member = member(&reopened, &objects, current)?;
    assert_eq!(
        term_positions(&reader, &old_member, old_pin.record(), &objects, first)?,
        (1, vec![0])
    );
    assert_eq!(
        term_positions_case(
            &reader,
            &old_member,
            old_pin.record(),
            &objects,
            CaseMode::Folded,
            last,
        )?,
        (1, vec![129])
    );
    assert_eq!(
        term_positions_case(
            &reader,
            &current_member,
            current_pin.record(),
            &objects,
            CaseMode::Folded,
            last,
        )?,
        (2, vec![129, 130])
    );
    assert_eq!(
        objects.get(block_ref, &mut budget())?.payload,
        block.payload
    );
    old_pin.release()?;
    current_pin.release()?;
    drop(reopened_catalog);
    Ok(())
}

fn occurrence_block_lifecycle(source_before_profile: bool) -> TestResult {
    let directory = tempfile::tempdir()?;
    let observer = Arc::new(quanta_index_storage::ObjectIoObservation::default());
    let ((engine, objects, catalog, spy), text_spec) =
        native_text_services(directory.path(), 64, 64, &observer)?;
    let genesis = engine.create_workspace(workspace(), 1, &mut budget())?;
    let mut view = genesis;
    let mut first_record = None;
    for is_source in [source_before_profile, !source_before_profile] {
        if is_source {
            let operation = if source_before_profile {
                operation(1)
            } else {
                operation(2)
            };
            let record = engine.stage_source_text(
                engine.begin_input(operation)?,
                "entry.rs",
                b"alpha beta beta",
                SourceTextAdmission::Utf8 {
                    policy: Digest([31; 32]),
                },
                &mut budget(),
            )?;
            view = published(block_on(engine.submit(
                native_text_cost_request(
                    &engine,
                    view,
                    source(&engine, view)?,
                    operation,
                    1,
                    vec![OwnerMutation {
                        owner: owner(1),
                        expected: None,
                        next: Some(record),
                    }],
                )?,
                &mut budget(),
            ))?)?;
            first_record = Some(record);
        } else {
            let operation = if source_before_profile {
                operation(2)
            } else {
                operation(1)
            };
            let profile = engine.stage_profile(
                engine.begin_input(operation)?,
                &IndexProfile {
                    instances: vec![text_spec.clone()],
                },
                &mut budget(),
            )?;
            view = published(block_on(engine.submit(
                UpdateRequest {
                    operation,
                    expected_view: view,
                    kind: UpdateKind::Reconfigure { profile },
                },
                &mut budget(),
            ))?)?;
        }
    }
    let bootstrap = view;
    let first_record = first_record.ok_or("initial source record absent")?;
    let old_pin = engine.pin_view(ViewSelection::Exact(bootstrap), &budget())?;
    let old_member = member(&engine, &objects, bootstrap)?;
    let [old_alpha, old_beta, old_folded_alpha, old_folded_beta] =
        occurrence_children(&objects, &old_member)?;
    assert_eq!(old_alpha, old_beta);
    assert_eq!(old_alpha, old_folded_alpha);
    assert_eq!(old_alpha, old_folded_beta);
    let block = objects.get(old_alpha, &mut budget())?;
    assert!(block.registration.children.is_empty());
    assert!(block.payload.starts_with(b"QITXOC01"));
    let reader = NativeTextIndex::new(TextRecipe::new(Digest([31; 32])));
    assert_eq!(
        term_positions(&reader, &old_member, old_pin.record(), &objects, "alpha")?,
        (1, vec![0])
    );
    assert_eq!(
        term_positions(&reader, &old_member, old_pin.record(), &objects, "beta")?,
        (2, vec![1, 2])
    );
    assert_eq!(
        phrase_source(
            &reader,
            &old_member,
            old_pin.record(),
            &objects,
            "alpha beta"
        )?,
        first_record
    );

    let next_record = engine.stage_source_text(
        engine.begin_input(operation(3))?,
        "entry.rs",
        b"alpha beta beta beta",
        SourceTextAdmission::Utf8 {
            policy: Digest([31; 32]),
        },
        &mut budget(),
    )?;
    let current = published(block_on(engine.submit(
        native_text_cost_request(
            &engine,
            bootstrap,
            source(&engine, bootstrap)?,
            operation(3),
            2,
            vec![OwnerMutation {
                owner: owner(1),
                expected: Some(first_record),
                next: Some(next_record),
            }],
        )?,
        &mut budget(),
    ))?)?;
    let current_pin = engine.pin_view(ViewSelection::Exact(current), &budget())?;
    let current_member = member(&engine, &objects, current)?;
    let [
        current_alpha,
        current_beta,
        current_folded_alpha,
        current_folded_beta,
    ] = occurrence_children(&objects, &current_member)?;
    assert_eq!(current_alpha, old_alpha);
    assert_eq!(current_folded_alpha, old_folded_alpha);
    assert_ne!(current_beta, old_beta);
    assert_eq!(current_beta, current_folded_beta);
    let changed = objects.get(current_beta, &mut budget())?;
    assert!(changed.payload.starts_with(b"{\"positions\":"));
    assert_eq!(
        term_positions(
            &reader,
            &current_member,
            current_pin.record(),
            &objects,
            "alpha"
        )?,
        (1, vec![0])
    );
    assert_eq!(
        term_positions(
            &reader,
            &current_member,
            current_pin.record(),
            &objects,
            "beta"
        )?,
        (3, vec![1, 2, 3])
    );
    assert_eq!(
        phrase_source(
            &reader,
            &current_member,
            current_pin.record(),
            &objects,
            "alpha beta"
        )?,
        next_record
    );
    old_pin.validate()?;
    assert_eq!(
        term_positions(&reader, &old_member, old_pin.record(), &objects, "beta")?,
        (2, vec![1, 2])
    );
    old_pin.release()?;
    current_pin.release()?;
    drop(spy);
    drop(objects);
    drop(engine);
    drop(catalog);

    let ((reopened, objects, reopened_catalog, _spy), _spec) =
        native_text_services(directory.path(), 64, 64, &observer)?;
    let old_pin = reopened.pin_view(ViewSelection::Exact(bootstrap), &budget())?;
    let new_pin = reopened.pin_view(ViewSelection::Exact(current), &budget())?;
    let old_member = member(&reopened, &objects, bootstrap)?;
    let current_member = member(&reopened, &objects, current)?;
    assert_eq!(
        term_positions(&reader, &old_member, old_pin.record(), &objects, "beta")?,
        (2, vec![1, 2])
    );
    assert_eq!(
        term_positions(&reader, &current_member, new_pin.record(), &objects, "beta")?,
        (3, vec![1, 2, 3])
    );
    assert_eq!(
        objects.get(old_alpha, &mut budget())?.payload,
        block.payload
    );
    old_pin.release()?;

    // Remove the last live reference to the bootstrap block. The pinned
    // intermediate view must still keep its physical occurrence reachable.
    let final_record = reopened.stage_source_text(
        reopened.begin_input(operation(4))?,
        "entry.rs",
        b"gamma beta beta beta",
        SourceTextAdmission::Utf8 {
            policy: Digest([31; 32]),
        },
        &mut budget(),
    )?;
    let latest = published(block_on(reopened.submit(
        native_text_cost_request(
            &reopened,
            current,
            source(&reopened, current)?,
            operation(4),
            3,
            vec![OwnerMutation {
                owner: owner(1),
                expected: Some(next_record),
                next: Some(final_record),
            }],
        )?,
        &mut budget(),
    ))?)?;
    for sequence in 1..=4 {
        loop {
            if !reopened_catalog
                .release_terminal_custody(operation(sequence), 256, &mut budget())?
                .has_more
            {
                break;
            }
        }
    }
    let _floor = reopened_catalog.advance_exact_history_floor(
        workspace(),
        1,
        latest.sequence,
        &mut budget(),
    )?;
    let retired =
        reopened_catalog.retire_exact_history_page(workspace(), 1, None, 16, &mut budget())?;
    assert!(retired.blocked >= 1);
    new_pin.validate()?;
    assert_eq!(
        term_positions(
            &reader,
            &current_member,
            new_pin.record(),
            &objects,
            "alpha"
        )?,
        (1, vec![0])
    );
    assert_eq!(
        objects.get(old_alpha, &mut budget())?.payload,
        block.payload
    );
    new_pin.release()?;
    let retired =
        reopened_catalog.retire_exact_history_page(workspace(), 1, None, 16, &mut budget())?;
    assert!(retired.retired >= 1);
    let mut reclaimed = false;
    for _ in 0..128 {
        let page = reopened_catalog.reclaim_work_page(None, 8, &mut budget())?;
        if page.items.is_empty() {
            break;
        }
        for work in page.items {
            let _page = objects.reclaim_one(work, 32, &mut budget())?;
        }
        if matches!(
            objects.get(old_alpha, &mut budget()),
            Err(IndexError::CorruptObject(_))
        ) {
            reclaimed = true;
            break;
        }
    }
    assert!(reclaimed, "unreferenced occurrence block was not reclaimed");
    let latest_pin = reopened.pin_view(ViewSelection::Exact(latest), &budget())?;
    let latest_member = member(&reopened, &objects, latest)?;
    assert_eq!(
        term_positions(
            &reader,
            &latest_member,
            latest_pin.record(),
            &objects,
            "gamma"
        )?,
        (1, vec![0])
    );
    latest_pin.release()?;

    // The same selected source cut now drives both the retained incremental
    // instance and a new instance that must bootstrap from that cut. Their
    // physical artifact identities may differ; observable membership does not.
    let fresh_spec = InstanceSpec {
        instance: IndexInstanceId([47; 16]),
        ..text_spec.clone()
    };
    let parity_profile = reopened.stage_profile(
        reopened.begin_input(operation(5))?,
        &IndexProfile {
            instances: vec![text_spec.clone(), fresh_spec.clone()],
        },
        &mut budget(),
    )?;
    let parity_view = published(block_on(reopened.submit(
        UpdateRequest {
            operation: operation(5),
            expected_view: latest,
            kind: UpdateKind::Reconfigure {
                profile: parity_profile,
            },
        },
        &mut budget(),
    ))?)?;
    let parity_pin = reopened.pin_view(ViewSelection::Exact(parity_view), &budget())?;
    let incremental = member_by_instance(&objects, parity_pin.record(), text_spec.instance)?;
    let fresh = member_by_instance(&objects, parity_pin.record(), fresh_spec.instance)?;
    assert_eq!(incremental.input.root, fresh.input.root);
    assert_eq!(incremental.input.policy, fresh.input.policy);
    let stats = |binding: &MemberBinding| {
        reader.inspect_stats(
            binding,
            TextReadContext {
                ticket: parity_pin.record(),
                objects: objects.as_ref(),
                maps: objects.as_ref(),
                budget: &mut budget(),
            },
        )
    };
    assert_eq!(stats(&incremental)?, stats(&fresh)?);
    for binding in [&incremental, &fresh] {
        assert_eq!(
            reader.inspect_term_df(
                binding,
                CaseMode::Sensitive,
                "alpha",
                TextReadContext {
                    ticket: parity_pin.record(),
                    objects: objects.as_ref(),
                    maps: objects.as_ref(),
                    budget: &mut budget(),
                },
            )?,
            0
        );
    }
    for (term, expected) in [("gamma", (1, vec![0])), ("beta", (3, vec![1, 2, 3]))] {
        assert_eq!(
            term_positions(&reader, &incremental, parity_pin.record(), &objects, term)?,
            expected
        );
        assert_eq!(
            term_positions(&reader, &fresh, parity_pin.record(), &objects, term)?,
            expected
        );
    }
    for (text, phrase) in [("beta", false), ("gamma beta", true)] {
        let leaf = || {
            if phrase {
                LqLeaf::Phrase(text.into())
            } else {
                LqLeaf::Keyword(text.into())
            }
        };
        let incremental_rank =
            ranked_leaf(&reader, &incremental, parity_pin.record(), &objects, leaf())?;
        let fresh_rank = ranked_leaf(&reader, &fresh, parity_pin.record(), &objects, leaf())?;
        assert_eq!(incremental_rank.matched, 1);
        assert_eq!(incremental_rank.matched, fresh_rank.matched);
        assert_eq!(incremental_rank.hits, fresh_rank.hits);
    }
    parity_pin.release()?;
    Ok(())
}
