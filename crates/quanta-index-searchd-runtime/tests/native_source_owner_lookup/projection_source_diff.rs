//! Real UDS producer delta discovery; no arbitrary roots or manufactured AST.

use super::*;
use quanta_index_contract::{
    index::{
        ProducerCompleteness, ProjectionCertificate, ProjectionId, ProjectionOwnerContribution,
        ProjectionTransition,
    },
    native_protocol::NativeProjectionSourceDiff,
};

const PROJECTION: ProjectionId = ProjectionId([40; 16]);

fn diff(
    client: &NativeUdsClient,
    selection: ViewSelection,
    after: Option<StableOwnerId>,
    limit: u32,
) -> TestResult<NativeProjectionSourceDiff> {
    let reply = client.request(
        NativeIndexRequest::DiffProjectionSourceOwners {
            selection,
            projection: PROJECTION,
            producer: operation(1).producer,
            after,
            limit,
        },
        &budget(),
    )?;
    let NativeIndexResponse::ProjectionSourceDiff(page) = reply else {
        return Err(format!("unexpected source diff reply: {reply:?}").into());
    };
    Ok(page)
}

fn projection_baseline(
    client: &NativeUdsClient,
    view: ViewRef,
    source: ObjectRef,
) -> TestResult<(ViewRef, ObjectRef)> {
    let operation = OperationKey {
        producer: ProducerId([41; 16]),
        stream: StreamId([42; 16]),
        sequence: 1,
        ..operation(1)
    };
    let ingress = client.begin_input(operation, &budget())?;
    let source_cut = client
        .read_view(ViewSelection::Exact(view), &budget())?
        .source;
    let output =
        client.stage_projection_output(ingress, b"producer-certified output", &budget())?;
    let projection_owner = OwnerKey {
        producer: operation.producer,
        stable_id: owner(1).stable_id,
    };
    let contribution = client.stage_projection_owner(
        ingress,
        ProjectionOwnerContribution {
            projection: PROJECTION,
            producer: operation.producer,
            owner: projection_owner,
            source_owner: Some(owner(1)),
            source: Some(source),
            dependency_stamp: Digest([43; 32]),
            output: Some(output),
        },
        &budget(),
    )?;
    let owners = client.stage_mutations(
        ingress,
        [OwnerMutation {
            owner: projection_owner,
            expected: None,
            next: Some(contribution),
        }],
        &budget(),
    )?;
    let certificate = ProjectionCertificate {
        projection: PROJECTION,
        producer: operation.producer,
        prior: None,
        target_source: source_cut,
        schema: SchemaId([44; 16]),
        producer_policy: Digest([45; 32]),
        dependency_context: Digest([46; 32]),
        affected: owners,
    };
    let certificate_ref = client.stage_projection_certificate(ingress, certificate, &budget())?;
    let mut transition = ProjectionTransition {
        projection: PROJECTION,
        producer: operation.producer,
        expected: None,
        target_source: source_cut,
        schema: SchemaId([44; 16]),
        producer_policy: Digest([45; 32]),
        dependency_context: Digest([46; 32]),
        completeness: ProducerCompleteness::CompleteAffectedDomain {
            certificate: certificate_ref,
        },
        owners,
        events: vec![],
    };
    transition.events.push(SourceEvent {
        producer: operation.producer,
        stream: operation.stream,
        stream_epoch: operation.stream_epoch,
        predecessor: 0,
        sequence: 1,
        payload: transition.payload_digest()?,
    });
    let baseline = published(client.submit(
        UpdateRequest {
            operation,
            expected_view: view,
            kind: UpdateKind::ProjectionAdvance(Box::new(transition)),
        },
        &budget(),
    )?)?;
    Ok((baseline, contribution))
}

fn run_case() -> TestResult {
    let root = tempfile::tempdir()?;
    std::fs::set_permissions(root.path(), std::fs::Permissions::from_mode(0o700))?;
    let socket = root.path().join("diff.sock");
    let running = RunningService::start(NativeIndexService::bind(
        root.path(),
        &socket,
        limits(),
        descriptors(),
        resources(),
    )?);
    let client = NativeUdsClient::new(socket.clone());
    let NativeIndexResponse::Created(genesis) = client.request(
        NativeIndexRequest::CreateWorkspace {
            workspace: workspace(),
            epoch: 1,
        },
        &budget(),
    )?
    else {
        return Err("missing genesis".into());
    };
    let ingress = client.begin_input(operation(1), &budget())?;
    let profile = client.stage_profile(
        ingress,
        IndexProfile {
            instances: vec![instance(20), instance(30)],
        },
        &budget(),
    )?;
    let configured = published(client.submit(
        UpdateRequest {
            operation: operation(1),
            expected_view: genesis,
            kind: UpdateKind::Reconfigure { profile },
        },
        &budget(),
    )?)?;
    let ingress = client.begin_input(operation(2), &budget())?;
    let original = client.stage_source(
        ingress,
        "first.rs".into(),
        b"original",
        SourceTextAdmission::Excluded,
        &budget(),
    )?;
    let other = client.stage_source(
        ingress,
        "second.rs".into(),
        b"unchanged",
        SourceTextAdmission::Excluded,
        &budget(),
    )?;
    let owners = client.stage_mutations(
        ingress,
        [
            OwnerMutation {
                owner: owner(1),
                expected: None,
                next: Some(original),
            },
            OwnerMutation {
                owner: owner(2),
                expected: None,
                next: Some(other),
            },
        ],
        &budget(),
    )?;
    let mut transition = SourceTransition {
        expected_source: client
            .read_view(ViewSelection::Exact(configured), &budget())?
            .source,
        owners,
        events: vec![],
    };
    transition.events.push(SourceEvent {
        producer: operation(1).producer,
        stream: operation(1).stream,
        stream_epoch: 1,
        predecessor: 0,
        sequence: 1,
        payload: transition.owner_digest()?,
    });
    let first = published(client.submit(
        UpdateRequest {
            operation: ingress.operation,
            expected_view: configured,
            kind: UpdateKind::SourceAdvance(transition),
        },
        &budget(),
    )?)?;
    let initial = diff(&client, ViewSelection::Exact(first), None, 1)?;
    check!(
        initial.binding.is_none() && initial.has_more,
        "initial source diff invented a baseline"
    );
    check_eq!(
        initial.rows.as_slice(),
        &[OwnerMutation {
            owner: owner(1),
            expected: None,
            next: Some(original)
        }],
        "initial source diff differs"
    );
    let next = diff(
        &client,
        ViewSelection::Exact(initial.view),
        initial.next_after(),
        1,
    )?;
    check!(
        !next.has_more && next.binding.is_none(),
        "initial source diff continuation did not end"
    );
    check_eq!(
        next.source,
        initial.source,
        "source diff page changed its source cut"
    );
    check_eq!(
        next.rows.as_slice(),
        &[OwnerMutation {
            owner: owner(2),
            expected: None,
            next: Some(other)
        }],
        "source diff continuation skipped an owner"
    );
    let (baseline, contribution) = projection_baseline(&client, first, original)?;
    let aligned = diff(&client, ViewSelection::Exact(baseline), None, 1)?;
    check!(
        aligned.binding.is_some() && aligned.rows.as_slice().is_empty() && !aligned.has_more,
        "aligned projection returned changes"
    );

    let ingress = client.begin_input(operation(3), &budget())?;
    let replacement = client.stage_source(
        ingress,
        "first.rs".into(),
        b"replacement",
        SourceTextAdmission::Excluded,
        &budget(),
    )?;
    let updated = advance(
        &client,
        ingress,
        baseline,
        Some(original),
        Some(replacement),
        2,
    )?;
    let changed = diff(&client, ViewSelection::Exact(updated), None, 1)?;
    check_eq!(
        changed.binding,
        aligned.binding,
        "source advance replaced the prior projection stamp"
    );
    check_eq!(
        changed.rows.as_slice(),
        &[OwnerMutation {
            owner: owner(1),
            expected: Some(original),
            next: Some(replacement)
        }],
        "changed source lost its old/new binding"
    );
    let ingress = client.begin_input(operation(4), &budget())?;
    let deleted = advance(&client, ingress, updated, Some(replacement), None, 3)?;
    let deletion = diff(&client, ViewSelection::Exact(deleted), None, 1)?;
    check_eq!(
        deletion.rows.as_slice(),
        &[OwnerMutation {
            owner: owner(1),
            expected: Some(original),
            next: None
        }],
        "deletion compared against the latest source instead of projection SourceCut"
    );
    check_eq!(
        deletion.binding,
        aligned.binding,
        "deletion lost the retained projection source"
    );
    // Metadata completion must retain the same certified contribution across
    // Source advance and deletion. Exercise the actual SDK/UDS lookup, not a
    // constructed reply or the current Source owner map.
    for view in [baseline, updated, deleted] {
        let projection_owner = OwnerKey {
            producer: ProducerId([41; 16]),
            stable_id: owner(1).stable_id,
        };
        let missing_owner = OwnerKey {
            producer: projection_owner.producer,
            stable_id: owner(9).stable_id,
        };
        let lookup = client.lookup_projection_owners(
            ViewSelection::Exact(view),
            PROJECTION,
            vec![projection_owner, missing_owner],
            &budget(),
        )?;
        check_eq!(
            lookup.view,
            view,
            "projection lookup changed its exact view"
        );
        check_eq!(
            lookup.binding,
            aligned.binding,
            "projection lookup lost its certified binding"
        );
        check_eq!(
            lookup.rows.as_slice(),
            &[
                quanta_index_contract::native_protocol::NativeProjectionOwnerBinding {
                    owner: projection_owner,
                    contribution: Some(contribution),
                },
                quanta_index_contract::native_protocol::NativeProjectionOwnerBinding {
                    owner: missing_owner,
                    contribution: None,
                },
            ],
            "projection lookup changed its contribution or fabricated a missing owner"
        );
    }
    for (after, limit) in [(None, 0), (None, 129), (Some(owner(1).stable_id), 1)] {
        let invalid = client.request(
            NativeIndexRequest::DiffProjectionSourceOwners {
                selection: ViewSelection::Latest {
                    workspace: workspace(),
                    epoch: 1,
                },
                projection: PROJECTION,
                producer: operation(1).producer,
                after,
                limit,
            },
            &budget(),
        );
        check!(
            matches!(invalid, Err(NativeUdsError::Refused(ref error)) if error.code == NativeIndexErrorCode::InvalidInput),
            "invalid source diff range was accepted"
        );
    }
    running.stop()?;
    let reopened = RunningService::start(NativeIndexService::bind(
        root.path(),
        &socket,
        limits(),
        descriptors(),
        resources(),
    )?);
    check_eq!(
        diff(&client, ViewSelection::Exact(deleted), None, 1)?,
        deletion,
        "reopen changed source diff identity"
    );
    reopened.stop()?;
    Ok(())
}

#[test]
fn projection_source_diff_uses_retained_cut_for_update_delete_and_reopen() -> TestResult {
    let producer = std::thread::Builder::new()
        .stack_size(16 * 1024 * 1024)
        .spawn(|| run_case().map_err(|error| error.to_string()))?;
    producer
        .join()
        .map_err(|_panic| "source diff fixture panicked")?
        .map_err(Into::into)
}
