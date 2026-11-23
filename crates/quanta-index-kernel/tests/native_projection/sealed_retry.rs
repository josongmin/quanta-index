//! An ordinary projection request can survive the seal-to-admission crash cut.

use super::*;

#[test]
fn sealed_projection_ingress_replays_exact_request_after_admission_cut() -> TestResult {
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
    let request = projection_request(
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
    let UpdateKind::ProjectionAdvance(transition) = &request.kind else {
        return Err("fixture did not construct a projection advance".into());
    };
    let ProducerCompleteness::CompleteAffectedDomain { certificate } = transition.completeness
    else {
        return Err("fixture did not stage a complete affected certificate".into());
    };
    let ingress = kernel.begin_input(request.operation)?;
    let mut children: Vec<_> = [transition.owners.root.0, Some(certificate)]
        .into_iter()
        .flatten()
        .collect();
    children.sort_unstable();
    children.dedup();
    let stored = objects.put(&serde_json::to_vec(&request)?, &children, &mut budget())?;
    catalog.reserve_inputs(ingress, std::slice::from_ref(&stored))?;
    catalog.confirm_inputs(ingress, std::slice::from_ref(&stored))?;
    catalog.seal_inputs(ingress, stored.reference)?;
    assert_eq!(
        kernel.status(
            request.operation,
            quanta_index_core::index::ports::OperationInspectAccess::Request(&budget()),
        )?,
        OperationStatus::Unknown,
        "the crash cut must precede operation admission",
    );
    drop(kernel);
    drop(objects);
    drop(catalog);

    let (reopened, _objects, _catalog) = setup(directory.path())?;
    let mut foreign = request.clone();
    foreign.expected_view = genesis;
    let foreign_while_sealed = block_on(reopened.submit(foreign, &mut budget()));
    assert_eq!(
        reopened.status(
            request.operation,
            quanta_index_core::index::ports::OperationInspectAccess::Request(&budget()),
        )?,
        OperationStatus::Unknown,
        "foreign bytes must not admit the sealed operation",
    );
    let view = published(block_on(reopened.submit(request.clone(), &mut budget()))?)?;
    assert_eq!(
        block_on(reopened.submit(request.clone(), &mut budget()))?,
        SubmitOutcome::Terminal(OperationStatus::Published(view)),
    );
    assert_eq!(
        foreign_while_sealed,
        Err(IndexError::DigestConflict),
        "another canonical request cannot replace the sealed request",
    );
    let mut incompatible = request;
    let UpdateKind::ProjectionAdvance(transition) = &mut incompatible.kind else {
        return Err("fixture lost its projection advance".into());
    };
    transition.dependency_context = Digest([26; 32]);
    assert_eq!(
        block_on(reopened.submit(incompatible, &mut budget())),
        Err(IndexError::DigestConflict),
        "a sealed operation cannot replay different request bytes",
    );
    Ok(())
}
