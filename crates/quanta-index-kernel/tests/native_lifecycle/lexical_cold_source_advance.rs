//! Cheap cold Text publication checks at the product's owner and packet boundaries.

use super::*;
use quanta_index_core::index::SourceTextAdmission;
use quanta_index_lexical::native::{CaseMode, NativeTextIndex, TextReadContext, TextRecipe};
use std::collections::BTreeMap;

fn product_owner(seed: u32) -> TestResult<OwnerKey> {
    let name = format!("row-{seed}");
    let mut stable = [0_u8; 16];
    stable
        .get_mut(..name.len())
        .ok_or("product-shaped owner exceeds StableOwnerId")?
        .copy_from_slice(name.as_bytes());
    Ok(OwnerKey {
        producer: operation(2).producer,
        stable_id: StableOwnerId(stable),
    })
}

fn check_cold_source_advance(rows: u32) -> TestResult {
    let directory = tempfile::tempdir()?;
    let io = Arc::new(quanta_index_storage::ObjectIoObservation::default());
    // The production Text adapter's default cold inventory window is 256.
    let ((engine, objects, _catalog, spy), text_spec) =
        native_text_services_with_batch_policy(directory.path(), 64, None, &io)?;
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

    let ingress = engine.begin_input(operation(2))?;
    let admission = SourceTextAdmission::Utf8 {
        policy: Digest([31; 32]),
    };
    let mut staged = Vec::new();
    for start in (0..rows).step_by(128) {
        let inputs = (start..rows.min(start.saturating_add(128)))
            .map(|seed| SourceInput {
                path: format!("src/{seed:04}.rs"),
                bytes: format!("row-{seed}").into_bytes(),
                text: admission,
                language: None,
            })
            .collect();
        staged.extend(engine.stage_sources(ingress, inputs, &mut budget())?);
    }
    assert_eq!(staged.len(), usize::try_from(rows)?);
    let mut expected: Vec<_> = (0..rows)
        .zip(staged)
        .map(|(seed, source)| Ok((product_owner(seed)?, source)))
        .collect::<TestResult<_>>()?;
    expected.sort_unstable_by_key(|(owner, _source)| *owner);
    assert!(
        expected
            .windows(2)
            .all(|pair| matches!(pair, [left, right] if left.0 < right.0))
    );
    let prior = source(&engine, configured)?;
    let request = native_text_cost_request(
        &engine,
        configured,
        prior,
        operation(2),
        1,
        expected
            .iter()
            .copied()
            .map(|(owner, record)| OwnerMutation {
                owner,
                expected: None,
                next: Some(record),
            })
            .collect(),
    )?;

    spy.arm(prior.owners, std::collections::BTreeSet::new())?;
    let submit_deadline = Instant::now()
        .checked_add(Duration::from_secs(30))
        .ok_or("cold SourceAdvance deadline overflow")?;
    let mut work = WorkBudget::new(
        WorkLimits {
            reads: 1_000_000,
            writes: 1_000_000,
            bytes: 512 * 1024 * 1024,
            mutations: 1_000_000,
        },
        submit_deadline,
    );
    let selected = published(block_on(engine.submit(request, &mut work))?)?;
    assert!(Instant::now() < submit_deadline);
    let usage = work.usage();
    let observed = spy.disarm()?;
    assert_eq!(observed.prior_source_scans, 0);
    assert_eq!(observed.unrelated_record_reads, 0);
    // Each distinct body contributes one packed occurrence, with 256-owner
    // physical batches. This tests the cold path; patch cost has another gate.
    assert_eq!(observed.occurrence_inputs, u64::from(rows));
    assert_eq!(
        observed.occurrence_install_calls,
        u64::from(rows).div_ceil(256)
    );
    // Tree Text also publishes one path posting root. Each src/NNNN.rs
    // contributes three path tokens in both case surfaces to that stream.
    assert_eq!(
        observed.single_map_stream.shape(),
        [1, 1, u64::from(rows).saturating_mul(6)]
    );
    assert!(observed.multi_map_stream.calls > 0);
    std::io::Write::write_fmt(
        &mut std::io::stdout().lock(),
        format_args!(
            "native-text-cold rows={rows} reads={} writes={} bytes={} mutations={} occurrence_inputs={} occurrence_calls={} multi_map_calls={} multi_map_edits={} scanned_entries={}\n",
            usage.reads,
            usage.writes,
            usage.bytes,
            usage.mutations,
            observed.occurrence_inputs,
            observed.occurrence_install_calls,
            observed.multi_map_stream.calls,
            observed.multi_map_stream.edits,
            observed.scanned_entries,
        ),
    )?;

    let ticket = engine.pin_view(ViewSelection::Exact(selected), &budget())?;
    let selected_source = ticket.view().source;
    assert_eq!(
        map_count(&objects, selected_source.owners)?,
        u64::from(rows)
    );
    for (owner, record) in &expected {
        assert_eq!(
            objects.lookup_object(
                selected_source.owners,
                &owner_map_key(*owner),
                &mut budget(),
            )?,
            Some(*record),
        );
    }
    let binding = member(&engine, &objects, selected)?;
    assert_eq!(binding.input.root, selected_source.owners);
    let reader = NativeTextIndex::new(TextRecipe::new(Digest([31; 32])));
    let stats = reader.inspect_stats(
        &binding,
        TextReadContext {
            ticket: ticket.record(),
            objects: objects.as_ref(),
            maps: objects.as_ref(),
            budget: &mut budget(),
        },
    )?;
    assert_eq!(stats.docs, u64::from(rows));
    assert_eq!(stats.indexed_tokens, u64::from(rows).saturating_mul(2));

    let mut observed_postings = BTreeMap::new();
    let scan = reader.visit_term(
        &binding,
        CaseMode::Sensitive,
        "row",
        TextReadContext {
            ticket: ticket.record(),
            objects: objects.as_ref(),
            maps: objects.as_ref(),
            budget: &mut budget(),
        },
        |hit| {
            if observed_postings.insert(hit.owner, hit.source).is_some()
                || hit.tf != 1
                || hit.doc_indexed_tokens != 2
                || hit.positions != [0]
            {
                return Err(IndexError::BackendViolation(
                    "product-shaped cold posting differs from fixed term gold".into(),
                ));
            }
            Ok(())
        },
    )?;
    assert_eq!(scan.df, u64::from(rows));
    let expected_postings: BTreeMap<OwnerKey, ObjectRef> = expected.iter().copied().collect();
    assert_eq!(observed_postings, expected_postings);
    for seed in 0..rows {
        let owner = product_owner(seed)?;
        let path = format!("src/{seed:04}.rs");
        assert_eq!(path_owner(&objects, selected_source, &path)?, owner);
        let record_ref = observed_postings
            .get(&owner)
            .ok_or("cold posting omitted a product-shaped owner")?;
        let record: SourceRecord =
            serde_json::from_slice(&objects.get(*record_ref, &mut budget())?.payload)?;
        assert_eq!(record.path, path);
        assert_eq!(
            objects.get(record.bytes, &mut budget())?.payload,
            format!("row-{seed}").into_bytes(),
        );
    }
    for seed in [0, 63, 64, 255, 256]
        .into_iter()
        .filter(|seed| *seed < rows)
    {
        let owner = product_owner(seed)?;
        let expected_source = expected_postings
            .get(&owner)
            .copied()
            .ok_or("cold posting fixture omitted a boundary owner")?;
        let mut hits = 0_u32;
        let scan = reader.visit_term(
            &binding,
            CaseMode::Sensitive,
            &seed.to_string(),
            TextReadContext {
                ticket: ticket.record(),
                objects: objects.as_ref(),
                maps: objects.as_ref(),
                budget: &mut budget(),
            },
            |hit| {
                if hit.owner != owner
                    || hit.source != expected_source
                    || hit.tf != 1
                    || hit.positions != [1]
                {
                    return Err(IndexError::BackendViolation(format!(
                        "cold numeric posting for {seed} differs from fixed gold"
                    )));
                }
                hits = hits.saturating_add(1);
                Ok(())
            },
        )?;
        assert_eq!((scan.df, hits), (1, 1), "cold numeric posting for {seed}");
    }
    ticket.release()?;
    Ok(())
}

#[test]
fn product_shaped_cold_text_source_advance_preserves_boundary_work_and_postings() -> TestResult {
    for rows in [63, 64, 65, 255, 256, 257] {
        check_cold_source_advance(rows)?;
    }
    Ok(())
}
