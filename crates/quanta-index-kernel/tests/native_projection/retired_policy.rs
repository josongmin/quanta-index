use super::*;

fn retired_policy() -> Digest {
    quanta_index_contract::native_protocol::native_repomap_rank_policy()
}

fn retired_producer() -> ProducerId {
    ProducerId(*b"semantica-r5-v1!")
}

#[test]
fn retired_policy_is_rejected_before_profile_certificate_or_submission_staging() -> TestResult {
    let directory = tempfile::tempdir()?;
    let (kernel, objects, catalog) = setup(directory.path())?;
    let genesis = kernel.create_workspace(workspace(), 1, &mut budget())?;
    let ingress = kernel.begin_input(runtime_operation(1))?;
    let mut retired_spec = spec();
    retired_spec.policy = retired_policy();
    assert_eq!(
        kernel.stage_profile(
            ingress,
            &IndexProfile {
                instances: vec![retired_spec]
            },
            &mut budget()
        ),
        Err(IndexError::InvalidInput(
            "retired RepoMap projection identity".into()
        )),
    );
    let mut retired_producer_spec = spec();
    retired_producer_spec.input_domain = InputDomain::Projection {
        projection: PROJECTION,
        producer: retired_producer(),
    };
    assert_eq!(
        kernel.stage_profile(
            ingress,
            &IndexProfile {
                instances: vec![retired_producer_spec]
            },
            &mut budget()
        ),
        Err(IndexError::InvalidInput(
            "retired RepoMap projection identity".into()
        )),
    );

    let source = current(&kernel)?.source;
    let certificate = ProjectionCertificate {
        projection: PROJECTION,
        producer: RUNTIME_PRODUCER,
        prior: None,
        target_source: source,
        schema: SchemaId([23; 16]),
        producer_policy: retired_policy(),
        dependency_context: Digest([25; 32]),
        affected: OwnerMutationSet {
            root: MapRoot(None),
            count: 0,
            digest: Digest([0; 32]),
        },
    };
    assert_eq!(
        kernel.stage_projection_certificate(ingress, &certificate, &mut budget()),
        Err(IndexError::InvalidInput(
            "retired RepoMap projection identity".into()
        )),
    );
    let mut mixed_certificate = certificate.clone();
    mixed_certificate.producer_policy = Digest([24; 32]);
    mixed_certificate.producer = retired_producer();
    assert_eq!(
        kernel.stage_projection_certificate(ingress, &mixed_certificate, &mut budget()),
        Err(IndexError::InvalidInput(
            "retired RepoMap projection identity".into()
        )),
    );
    let retired_owner = ProjectionOwnerContribution {
        projection: PROJECTION,
        producer: retired_producer(),
        owner: owner(retired_producer(), 1),
        source_owner: None,
        source: None,
        dependency_stamp: Digest([0; 32]),
        output: None,
    };
    assert_eq!(
        kernel.stage_projection_owner(ingress, retired_owner, &mut budget()),
        Err(IndexError::InvalidInput(
            "retired RepoMap projection identity".into()
        )),
    );

    let profile = kernel.stage_profile(
        ingress,
        &IndexProfile {
            instances: vec![spec()],
        },
        &mut budget(),
    )?;
    let configured = published(block_on(kernel.submit(
        UpdateRequest {
            operation: runtime_operation(1),
            expected_view: genesis,
            kind: UpdateKind::Reconfigure { profile },
        },
        &mut budget(),
    ))?)?;
    let mut request = projection_request(
        &kernel,
        &catalog,
        &objects,
        &ProjectionRequestParams {
            view: configured,
            source: current(&kernel)?.source,
            prior: None,
            operation_sequence: 2,
            event_sequence: 1,
            schema: SchemaId([23; 16]),
            producer_policy: Digest([24; 32]),
            owners: Vec::new(),
        },
    )?;
    let UpdateKind::ProjectionAdvance(transition) = &mut request.kind else {
        return Err("fixture did not construct a projection advance".into());
    };
    transition.producer_policy = retired_policy();
    assert_eq!(
        block_on(kernel.submit(request.clone(), &mut budget())),
        Err(IndexError::InvalidInput(
            "retired RepoMap projection identity".into()
        )),
    );
    if let UpdateKind::ProjectionAdvance(transition) = &mut request.kind {
        transition.producer_policy = Digest([24; 32]);
        transition.producer = retired_producer();
    }
    assert_eq!(
        block_on(kernel.submit(request.clone(), &mut budget())),
        Err(IndexError::InvalidInput(
            "retired RepoMap projection identity".into()
        )),
    );
    if let UpdateKind::ProjectionAdvance(transition) = &mut request.kind {
        transition.producer = RUNTIME_PRODUCER;
    }
    request.operation.producer = retired_producer();
    assert_eq!(
        block_on(kernel.submit(request.clone(), &mut budget())),
        Err(IndexError::InvalidInput(
            "retired RepoMap projection identity".into()
        )),
    );
    assert_eq!(
        kernel.status(
            request.operation,
            quanta_index_core::index::ports::OperationInspectAccess::Request(&budget())
        )?,
        OperationStatus::Unknown,
    );
    assert_eq!(current(&kernel)?.identity, configured);
    Ok(())
}

#[test]
fn pending_retired_projection_cannot_rotate_recovery_or_publish() -> TestResult {
    let directory = tempfile::tempdir()?;
    let (kernel, objects, catalog) = setup(directory.path())?;
    let genesis = kernel.create_workspace(workspace(), 1, &mut budget())?;
    let profile = stage_profile(&kernel, runtime_operation(1), vec![spec()])?;
    let configured = published(block_on(kernel.submit(
        UpdateRequest {
            operation: runtime_operation(1),
            expected_view: genesis,
            kind: UpdateKind::Reconfigure { profile },
        },
        &mut budget(),
    ))?)?;
    let mut request = projection_request(
        &kernel,
        &catalog,
        &objects,
        &ProjectionRequestParams {
            view: configured,
            source: current(&kernel)?.source,
            prior: None,
            operation_sequence: 2,
            event_sequence: 1,
            schema: SchemaId([23; 16]),
            producer_policy: Digest([24; 32]),
            owners: Vec::new(),
        },
    )?;
    let UpdateKind::ProjectionAdvance(transition) = &mut request.kind else {
        return Err("fixture did not construct a projection advance".into());
    };
    transition.producer_policy = retired_policy();
    let retired_certificate = ProjectionCertificate {
        projection: transition.projection,
        producer: transition.producer,
        prior: transition.expected,
        target_source: transition.target_source,
        schema: transition.schema,
        producer_policy: transition.producer_policy,
        dependency_context: transition.dependency_context,
        affected: transition.owners,
    };
    let certificate = objects.put(
        &serde_json::to_vec(&retired_certificate)?,
        &retired_certificate.object_children(),
        &mut budget(),
    )?;
    transition.completeness = ProducerCompleteness::CompleteAffectedDomain {
        certificate: certificate.reference,
    };
    transition.events[0].payload = transition.payload_digest()?;

    // Simulate an admitted request written by the previous binary. Bypass the
    // current Kernel staging gate only to construct the durable regression.
    let ingress = kernel.begin_input(request.operation)?;
    let mut seen = std::collections::BTreeSet::new();
    adopt_fixture_tree(
        &catalog,
        &objects,
        ingress,
        certificate.reference,
        &mut seen,
    )?;
    let children = [transition.owners.root.0, Some(certificate.reference)]
        .into_iter()
        .flatten()
        .collect::<Vec<_>>();
    let stored = objects.put(&serde_json::to_vec(&request)?, &children, &mut budget())?;
    catalog.reserve_inputs(ingress, std::slice::from_ref(&stored))?;
    catalog.confirm_inputs(ingress, std::slice::from_ref(&stored))?;
    catalog.seal_inputs(ingress, stored.reference)?;
    let immutable = ImmutableOperation {
        key: request.operation,
        request: stored.reference,
        source_mode: request.kind.source_mode(),
    };
    let UpdateKind::ProjectionAdvance(transition) = &request.kind else {
        return Err("fixture lost projection advance".into());
    };
    let admitted = catalog.admit_projection(
        immutable,
        ingress,
        &transition.events,
        quanta_index_contract::index::ProjectionReplayStamp {
            expected_view: request.expected_view,
            projection: transition.projection,
            source: transition.target_source,
            context: transition.dependency_context,
        },
        &mut budget(),
    )?;
    let AdmissionOutcome::New(attempt) = admitted else {
        return Err(format!("expected pending admission, got {admitted:?}").into());
    };
    let before = OperationStatus::Pending(attempt);
    assert_eq!(
        kernel.status(
            request.operation,
            quanta_index_core::index::ports::OperationInspectAccess::Request(&budget())
        )?,
        before
    );

    assert_eq!(
        kernel.resume_dispatch(request.operation, &mut budget()),
        Err(IndexError::InvalidInput(
            "retired RepoMap projection identity".into()
        )),
    );
    assert_eq!(
        block_on(kernel.resume(request.operation, &mut budget())),
        Err(IndexError::InvalidInput(
            "retired RepoMap projection identity".into()
        )),
    );
    assert_eq!(
        kernel.status(
            request.operation,
            quanta_index_core::index::ports::OperationInspectAccess::Request(&budget())
        )?,
        before
    );
    assert_eq!(current(&kernel)?.identity, configured);
    Ok(())
}
