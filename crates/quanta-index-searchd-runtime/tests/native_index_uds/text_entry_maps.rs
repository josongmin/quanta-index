//! Real Native Text `SourceAdvance` and query path across bounded entry-map shapes.

use super::*;

fn configured_text_view(
    client: &NativeUdsClient,
) -> TestResult<quanta_index_contract::index::ViewRef> {
    let NativeIndexResponse::Created(genesis) = client.request(
        NativeIndexRequest::CreateWorkspace {
            workspace: workspace(),
            epoch: 1,
        },
        &budget(),
    )?
    else {
        return Err("text entry-map genesis reply differs".into());
    };
    let ingress = client.begin_input(operation(1), &budget())?;
    let profile = client.stage_profile(
        ingress,
        IndexProfile {
            instances: vec![native_text_spec()],
        },
        &budget(),
    )?;
    published(&client.submit(
        UpdateRequest {
            operation: operation(1),
            expected_view: genesis,
            kind: UpdateKind::Reconfigure { profile },
        },
        &budget(),
    )?)
}

fn advance_text_owners(
    client: &NativeUdsClient,
    prior_view: quanta_index_contract::index::ViewRef,
    sequence: u64,
    ingress: IngressRef,
    mutations: Vec<OwnerMutation>,
) -> TestResult<quanta_index_contract::index::ViewRef> {
    let prior = client.read_view(ViewSelection::Exact(prior_view), &budget())?;
    let owners = client.stage_mutations(ingress, mutations, &budget())?;
    let event_sequence = sequence
        .checked_sub(1)
        .ok_or("text event sequence underflow")?;
    let mut transition = SourceTransition {
        expected_source: prior.source,
        owners,
        events: Vec::new(),
    };
    transition.events.push(SourceEvent {
        producer: operation(sequence).producer,
        stream: operation(sequence).stream,
        stream_epoch: 1,
        predecessor: event_sequence
            .checked_sub(1)
            .ok_or("text event predecessor underflow")?,
        sequence: event_sequence,
        payload: transition.owner_digest()?,
    });
    published(&client.submit(
        UpdateRequest {
            operation: operation(sequence),
            expected_view: prior_view,
            kind: UpdateKind::SourceAdvance(transition),
        },
        &budget(),
    )?)
}

fn query_ids(
    client: &NativeUdsClient,
    view: quanta_index_contract::index::ViewRef,
    term: &str,
    expected: &[u8],
) -> TestResult<quanta_index_contract::native_protocol::NativeTextResult> {
    let result = client.search_text(
        ViewSelection::Exact(view),
        native_text_spec().instance,
        native_text_request(term, 16),
        &budget(),
    )?;
    assert_eq!(result.view, view);
    assert!(result.exhaustive);
    assert_eq!(result.matched, u64::try_from(expected.len())?);
    assert_eq!(result.hits.len(), expected.len());
    let mut ids = result
        .hits
        .iter()
        .map(|hit| hit.owner.stable_id.0)
        .collect::<Vec<_>>();
    ids.sort_unstable();
    let expected = expected.iter().map(|id| [*id; 16]).collect::<Vec<_>>();
    assert_eq!(ids, expected);
    Ok(result)
}

#[test]
fn block_queries_preserve_fixed_gold_across_delta_delete_noop_and_reopen() -> TestResult {
    let root = private_root()?;
    let socket = root.path().join("native.sock");
    let service = NativeServiceGuard::start(bind_native_text_service(root.path(), &socket)?);
    let client = NativeUdsClient::new(socket.clone());
    let configured = configured_text_view(&client)?;
    let ingress = client.begin_input(operation(2), &budget())?;
    // Seven distinct words yield more than eight inventory keys for owner 1.
    // "needle" has df=9, then df=8 after deletion; "solo" has df=1.
    let sources = (1_u8..=9)
        .map(|id| SourceInput {
            path: format!("owner-{id:02}.rs"),
            bytes: if id == 1 {
                b"alpha bravo cedar delta echo needle solo".to_vec()
            } else {
                b"needle".to_vec()
            },
            text: SourceTextAdmission::Utf8 {
                policy: Digest([31; 32]),
            },
        })
        .collect::<Vec<_>>();
    let staged = client.stage_sources(ingress, sources, &budget())?;
    assert_eq!(staged.len(), 9);
    let mutations = staged
        .iter()
        .enumerate()
        .map(|(slot, reference)| -> TestResult<OwnerMutation> {
            let ordinal = slot.checked_add(1).ok_or("text fixture ordinal overflow")?;
            let stable = u8::try_from(ordinal)?;
            Ok(OwnerMutation {
                owner: OwnerKey {
                    producer: operation(2).producer,
                    stable_id: StableOwnerId([stable; 16]),
                },
                expected: None,
                next: Some(*reference),
            })
        })
        .collect::<TestResult<Vec<_>>>()?;
    let nine = advance_text_owners(&client, configured, 2, ingress, mutations)?;
    let nine_needle = query_ids(&client, nine, "needle", &[1, 2, 3, 4, 5, 6, 7, 8, 9])?;
    let first_needle = nine_needle.hits.first().ok_or("first needle hit absent")?;
    let last_needle = nine_needle.hits.get(8).ok_or("ninth needle hit absent")?;
    assert_eq!(first_needle.owner.stable_id, StableOwnerId([2; 16]));
    assert_eq!(last_needle.owner.stable_id, StableOwnerId([1; 16]));
    assert!(first_needle.score.get() > last_needle.score.get());
    let solo = query_ids(&client, nine, "solo", &[1])?;
    assert!(solo.hits.first().ok_or("solo hit absent")?.score.get() > 0.0);

    let delete_ingress = client.begin_input(operation(3), &budget())?;
    let eight = advance_text_owners(
        &client,
        nine,
        3,
        delete_ingress,
        vec![OwnerMutation {
            owner: OwnerKey {
                producer: operation(3).producer,
                stable_id: StableOwnerId([9; 16]),
            },
            expected: Some(*staged.get(8).ok_or("deleted source absent")?),
            next: None,
        }],
    )?;
    let eight_needle = query_ids(&client, eight, "needle", &[1, 2, 3, 4, 5, 6, 7, 8])?;
    assert_ne!(eight_needle.hits, nine_needle.hits);
    assert_eq!(
        query_ids(&client, nine, "needle", &[1, 2, 3, 4, 5, 6, 7, 8, 9])?.hits,
        nine_needle.hits
    );

    let noop_ingress = client.begin_input(operation(4), &budget())?;
    let noop = advance_text_owners(&client, eight, 4, noop_ingress, Vec::new())?;
    assert_eq!(
        query_ids(&client, noop, "needle", &[1, 2, 3, 4, 5, 6, 7, 8])?.hits,
        eight_needle.hits
    );
    service.stop()?;

    let reopened = NativeServiceGuard::start(bind_native_text_service(root.path(), &socket)?);
    assert_eq!(
        query_ids(&client, noop, "needle", &[1, 2, 3, 4, 5, 6, 7, 8])?.hits,
        eight_needle.hits
    );
    assert_eq!(query_ids(&client, nine, "solo", &[1])?.hits, solo.hits);
    reopened.stop()
}

#[test]
fn old_layout_policy_and_foreign_format_refuse_before_native_text_work() -> TestResult {
    let root = private_root()?;
    let socket = root.path().join("native.sock");
    let service = NativeServiceGuard::start(bind_native_text_service(root.path(), &socket)?);
    let client = NativeUdsClient::new(socket);
    let NativeIndexResponse::Created(genesis) = client.request(
        NativeIndexRequest::CreateWorkspace {
            workspace: workspace(),
            epoch: 1,
        },
        &budget(),
    )?
    else {
        return Err("text policy genesis reply differs".into());
    };
    // Fixed pre-QITXEM01 recipe digest, independently computed from the prior
    // public TextRecipe fields. It must not be recomputed from the current recipe.
    let old_policy = Digest([
        0xa4, 0x24, 0xe2, 0x98, 0xce, 0x4a, 0x78, 0x03, 0xb8, 0x7d, 0xf2, 0xbb, 0xf2, 0x04, 0x2d,
        0x06, 0x7e, 0x03, 0xcd, 0x14, 0x4a, 0xfe, 0xdc, 0x1c, 0x7a, 0x9a, 0xa3, 0xae, 0x26, 0x53,
        0x77, 0x2e,
    ]);
    assert_ne!(old_policy, native_text_spec().policy);
    let mut stale = native_text_spec();
    stale.policy = old_policy;
    let ingress = client.begin_input(operation(1), &budget())?;
    assert!(matches!(
        client.stage_profile(ingress, IndexProfile { instances: vec![stale] }, &budget()),
        Err(NativeUdsError::Refused(error)) if error.code == NativeIndexErrorCode::InvalidInput
    ));
    let mut foreign = native_text_spec();
    foreign.format = StorageFormatId([76; 16]);
    assert!(matches!(
        client.stage_profile(ingress, IndexProfile { instances: vec![foreign] }, &budget()),
        Err(NativeUdsError::Refused(error)) if error.code == NativeIndexErrorCode::InvalidInput
    ));
    let after = client.read_view(
        ViewSelection::Latest {
            workspace: workspace(),
            epoch: 1,
        },
        &budget(),
    )?;
    assert_eq!(
        after,
        client.read_view(ViewSelection::Exact(genesis), &budget())?
    );
    service.stop()
}
