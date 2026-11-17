//! Actual Q publications only; no signed Source/Host/ACK fixture is issued.
use super::*;
use quanta_index_contract::index::{
    CatalogView, MapRoot, ObjectRef, ProducerCompleteness, ProjectionCertificate, ProjectionId,
    ProjectionOwnerContribution, ProjectionTransition,
};

#[derive(Clone, Copy)]
enum Intervening {
    None,
    SameProfile,
    Source,
    Profile,
}

fn advance_source_with_capture(
    fixture: &PreparedNativeFixture,
) -> TestResult<(ViewRef, ObjectRef, UpdateRequest)> {
    let input = fixture.client.begin_input(operation(3), &request())?;
    let sources = fixture.client.stage_sources(
        input,
        vec![SourceInput {
            path: "legacy-negative.rs".into(),
            bytes: b"different actual Source after FULL".to_vec(),
            text: SourceTextAdmission::Utf8 {
                policy: Digest([31; 32]),
            },
        }],
        &request(),
    )?;
    let [record] = sources.as_slice() else {
        return Err("source fixture count differs".into());
    };
    let owners = fixture.client.stage_mutations(
        input,
        [OwnerMutation {
            owner: OwnerKey {
                producer: operation(2).producer,
                stable_id: StableOwnerId([1; 16]),
            },
            expected: Some(fixture.record),
            next: Some(*record),
        }],
        &request(),
    )?;
    let current = fixture
        .client
        .read_view(ViewSelection::Exact(fixture.native_view), &request())?;
    let mut transition = SourceTransition {
        expected_source: current.source,
        owners,
        events: Vec::new(),
    };
    transition.events.push(SourceEvent {
        producer: operation(3).producer,
        stream: operation(3).stream,
        stream_epoch: 1,
        predecessor: 1,
        sequence: 2,
        payload: transition.owner_digest()?,
    });
    let captured = UpdateRequest {
        operation: operation(3),
        expected_view: fixture.native_view,
        kind: UpdateKind::SourceAdvance(transition),
    };
    let view = published(fixture.client.submit(captured.clone(), &request())?)?;
    Ok((view, *record, captured))
}
pub(super) fn advance_source(fixture: &PreparedNativeFixture) -> TestResult<(ViewRef, ObjectRef)> {
    let (view, record, _captured) = advance_source_with_capture(fixture)?;
    Ok((view, record))
}

fn change_profile(fixture: &PreparedNativeFixture, replace_instance: bool) -> TestResult<ViewRef> {
    let input = fixture.client.begin_input(operation(3), &request())?;
    let mut instance = text_spec();
    if replace_instance {
        instance.instance = IndexInstanceId([76; 16]);
    }
    let profile = fixture.client.stage_profile(
        input,
        IndexProfile {
            instances: vec![instance],
        },
        &request(),
    )?;
    published(fixture.client.submit(
        UpdateRequest {
            operation: operation(3),
            expected_view: fixture.native_view,
            kind: UpdateKind::Reconfigure { profile },
        },
        &request(),
    )?)
}

fn publish_ast(
    fixture: &PreparedNativeFixture,
    current: &CatalogView,
    record: ObjectRef,
) -> TestResult<(UpdateRequest, ViewRef)> {
    let projection = ProjectionId([20; 16]);
    let producer = ProducerId([15; 16]);
    let ast_operation = OperationKey {
        producer,
        stream: StreamId([7; 16]),
        sequence: 1,
        ..operation(1)
    };
    let input = fixture.client.begin_input(ast_operation, &request())?;
    let output =
        fixture
            .client
            .stage_projection_output(input, b"actual derived AST output", &request())?;
    let contribution = ProjectionOwnerContribution {
        projection,
        producer,
        owner: OwnerKey {
            producer,
            stable_id: StableOwnerId([1; 16]),
        },
        source_owner: Some(OwnerKey {
            producer: operation(2).producer,
            stable_id: StableOwnerId([1; 16]),
        }),
        source: Some(record),
        dependency_stamp: Digest([21; 32]),
        output: Some(output),
    };
    let contribution_ref =
        fixture
            .client
            .stage_projection_owner(input, contribution, &request())?;
    let owners = fixture.client.stage_mutations(
        input,
        [OwnerMutation {
            owner: contribution.owner,
            expected: None,
            next: Some(contribution_ref),
        }],
        &request(),
    )?;
    let mut transition = ProjectionTransition {
        projection,
        producer,
        expected: None,
        target_source: current.source,
        schema: SchemaId([22; 16]),
        producer_policy: Digest([23; 32]),
        dependency_context: Digest([24; 32]),
        completeness: ProducerCompleteness::Partial {
            missing: MapRoot(None),
        },
        owners,
        events: Vec::new(),
    };
    let certificate = fixture.client.stage_projection_certificate(
        input,
        ProjectionCertificate {
            projection,
            producer,
            prior: None,
            target_source: current.source,
            schema: transition.schema,
            producer_policy: transition.producer_policy,
            dependency_context: transition.dependency_context,
            affected: owners,
        },
        &request(),
    )?;
    transition.completeness = ProducerCompleteness::CompleteAffectedDomain { certificate };
    transition.events.push(SourceEvent {
        producer,
        stream: ast_operation.stream,
        stream_epoch: 1,
        predecessor: 0,
        sequence: 1,
        payload: transition.payload_digest()?,
    });
    let captured = UpdateRequest {
        operation: ast_operation,
        expected_view: current.identity,
        kind: UpdateKind::ProjectionAdvance(Box::new(transition)),
    };
    let view = published(fixture.client.submit(captured.clone(), &request())?)?;
    Ok((captured, view))
}

fn authenticate_full_ast(intervening: Intervening) -> TestResult {
    let fixture = prepare_native_fixture()?;
    let (current, record) = match intervening {
        Intervening::None => (fixture.native_view, fixture.record),
        Intervening::SameProfile => {
            let view = change_profile(&fixture, false)?;
            require(
                view != fixture.native_view,
                "intermediate actual publication did not advance the view",
            )?;
            (view, fixture.record)
        }
        Intervening::Source => advance_source(&fixture)?,
        Intervening::Profile => (change_profile(&fixture, true)?, fixture.record),
    };
    let current = fixture
        .client
        .read_view(ViewSelection::Exact(current), &request())?;
    let (ast_request, ast_view) = publish_ast(&fixture, &current, record)?;
    let projected = fixture
        .client
        .read_view(ViewSelection::Exact(ast_view), &request())?;
    require(
        projected.source == current.source,
        "actual AST changed SourceCut",
    )?;
    require(
        ast_view != fixture.native_view,
        "actual AST did not advance the view",
    )?;
    fixture.running.finish()?;
    let registry = compose_boot_registry(&fixture.config)?;
    let select = NativeStateCommandV1 {
        mode: NativeCommandModeV1::Select,
        config_path: fixture.config_path.clone(),
        identity: None,
        config_digest: Some(fixture.config_digest),
        install_catalog_digest: None,
        select_operation: Some(ast_request.operation),
        receipt_path: Some(fixture.receipt_path.clone()),
    };
    require(
        boot::execute(&select, &fixture.config, &registry, request())
            .map_err(|error| error.context(boot_fixture_failure_context(&fixture.root)))?
            .is_none(),
        "offline AST Select returned live authority",
    )?;
    let target = NativeCutoverEvidenceTargetV1 {
        operation: operation(2),
        expected_view: fixture.configured,
        expected_source: fixture.prior.source,
        expected_profile: fixture.profile,
        source_mode: SourceOperationMode::Advance,
        event_predecessor: 0,
        event_sequence: 1,
        text_admission: SourceTextAdmission::Utf8 {
            policy: Digest([31; 32]),
        },
        // Not supplied to Source joining or any Original permission gate.
        owner_identity_policy_digest: Digest([0; 32]),
        canonical_target_digest: Digest([0; 32]),
    };

    let result =
        boot::with_admitted_control_budget(&fixture.config, request(), false, |budget, _cache| {
            boot::with_boot_authority(&fixture.config, budget, |authority, budget| {
                let root = authority.lease.state_root_identity_v1();
                let full = quanta_index_catalog::verify_native_rollback_with_source_publications(
                    root,
                    authority.io.as_ref(),
                    operation(2),
                    target.operation,
                    budget,
                )?;
                let snapshot = full.snapshot();
                let selected = snapshot
                    .selected
                    .as_ref()
                    .ok_or(IndexError::SnapshotUnavailable)?;
                require(
                    selected.operation.key == ast_request.operation,
                    "BOOT did not select actual AST",
                )
                .map_err(|error| anyhow!(error.to_string()))?;
                let source = boot::RecoverySourceInputs {
                    target: &target,
                    baseline_candidate_root_nonce: Some(Digest(snapshot.root_nonce)),
                    request: &fixture.captured,
                    current_target: &target,
                    current_request: &fixture.captured,
                    legacy_publication: None,
                    joined_source: None,
                    ast_work: Some(boot::RecoveryAstInputs {
                        operation: ast_request.operation,
                        publication: ast_view,
                        // Expect FULL X even when the actual selected AST used another
                        // Source or profile. No signed producer census is fabricated.
                        source_view: fixture.native_view,
                        source_cut: projected.source,
                        request: &ast_request,
                    }),
                };
                let capture = boot::RecoveryCapture {
                    source: &source,
                    full: &full,
                };
                let result = boot::authenticate(
                    &authority,
                    snapshot,
                    selected,
                    &registry,
                    fixture.config.resources.physical.inventory.max_entries,
                    Some(&capture),
                    budget,
                );
                // A physical-retirement error must not become an expected negative.
                boot::require_retired(&authority)?;
                result
            })
        });
    match intervening {
        Intervening::None | Intervening::SameProfile => {
            result?;
            Ok(())
        }
        Intervening::Source | Intervening::Profile => match result {
            Err(error)
                if matches!(
                    error.downcast_ref::<IndexError>(),
                    Some(IndexError::DigestConflict)
                ) =>
            {
                Ok(())
            }
            Err(error) => Err(format!("wrong mixed FULL/AST refusal: {error:#}").into()),
            Ok(()) => Err("mixed FULL/AST publication was accepted".into()),
        },
    }
}

#[test]
fn actual_full_x_and_ast_y_share_the_same_historical_source_and_profile() -> TestResult {
    authenticate_full_ast(Intervening::None)
}
#[test]
fn actual_ast_after_another_source_cannot_authenticate_full_x() -> TestResult {
    authenticate_full_ast(Intervening::Source)
}
#[test]
fn actual_ast_after_another_profile_cannot_authenticate_full_x() -> TestResult {
    authenticate_full_ast(Intervening::Profile)
}

#[test]
fn actual_ast_expected_view_can_follow_a_same_source_profile_publication() -> TestResult {
    // This is a real same-profile Reconfigure view, not a claim of Semantic
    // materialization. It proves the AST expected view need not equal FULL X.
    authenticate_full_ast(Intervening::SameProfile)
}

#[test]
fn actual_selected_delta_uses_current_native_capture_and_refuses_historical_full_alias()
-> TestResult {
    let fixture = prepare_native_fixture()?;
    let (current_view, _record, current_request) = advance_source_with_capture(&fixture)?;
    let UpdateKind::SourceAdvance(current_transition) = &current_request.kind else {
        return Err("actual current request is not SourceAdvance".into());
    };
    let expected_source = current_transition.expected_source;
    let full_target = NativeCutoverEvidenceTargetV1 {
        operation: operation(2),
        expected_view: fixture.configured,
        expected_source: fixture.prior.source,
        expected_profile: fixture.profile,
        source_mode: SourceOperationMode::Advance,
        event_predecessor: 0,
        event_sequence: 1,
        text_admission: SourceTextAdmission::Utf8 {
            policy: Digest([31; 32]),
        },
        // Q-only captured request fixture; no signed producer permission.
        owner_identity_policy_digest: Digest([0; 32]),
        canonical_target_digest: Digest([0; 32]),
    };
    let current_target = NativeCutoverEvidenceTargetV1 {
        operation: operation(3),
        expected_view: fixture.native_view,
        expected_source,
        event_predecessor: 1,
        event_sequence: 2,
        ..full_target
    };
    fixture.running.finish()?;
    let registry = compose_boot_registry(&fixture.config)?;
    let select = NativeStateCommandV1 {
        mode: NativeCommandModeV1::Select,
        config_path: fixture.config_path.clone(),
        identity: None,
        config_digest: Some(fixture.config_digest),
        install_catalog_digest: None,
        select_operation: Some(operation(3)),
        receipt_path: Some(fixture.receipt_path.clone()),
    };
    require(
        boot::execute(&select, &fixture.config, &registry, request())?.is_none(),
        "actual DELTA BOOT selection returned a live owner",
    )?;
    boot::with_admitted_control_budget(&fixture.config, request(), false, |budget, _cache| {
        boot::with_boot_authority(&fixture.config, budget, |authority, budget| {
            let full = quanta_index_catalog::verify_native_rollback_with_source_publications(
                &fixture.root,
                authority.io.as_ref(),
                operation(2),
                operation(3),
                budget,
            )?;
            let snapshot = full.snapshot();
            let selected = snapshot
                .selected
                .as_ref()
                .ok_or(IndexError::SnapshotUnavailable)?;
            require(
                selected.operation.key == operation(3)
                    && selected.publication.identity == current_view,
                "actual selected DELTA metadata differs",
            )
            .map_err(|error| anyhow!(error.to_string()))?;
            let mut source = boot::RecoverySourceInputs {
                target: &full_target,
                baseline_candidate_root_nonce: Some(Digest(snapshot.root_nonce)),
                request: &fixture.captured,
                current_target: &current_target,
                current_request: &current_request,
                legacy_publication: None,
                joined_source: None,
                ast_work: None,
            };
            let capture = boot::RecoveryCapture {
                source: &source,
                full: &full,
            };
            boot::authenticate(
                &authority,
                snapshot,
                selected,
                &registry,
                fixture.config.resources.physical.inventory.max_entries,
                Some(&capture),
                budget,
            )?;
            boot::require_retired(&authority)?;
            // Q-only negative: preserve the actual publication/request and
            // change only its expected Source scope. No signed issuer is made.
            let mut different_nonce = snapshot.root_nonce;
            different_nonce[0] ^= 1;
            let mut accepted_nonce = [false; 2];
            for (case, expected_nonce) in [None, Some(Digest(different_nonce))]
                .into_iter()
                .enumerate()
            {
                source.baseline_candidate_root_nonce = expected_nonce;
                let missing_nonce = source.baseline_candidate_root_nonce.is_none();
                let capture = boot::RecoveryCapture {
                    source: &source,
                    full: &full,
                };
                let refused = boot::authenticate(
                    &authority,
                    snapshot,
                    selected,
                    &registry,
                    fixture.config.resources.physical.inventory.max_entries,
                    Some(&capture),
                    budget,
                );
                boot::require_retired(&authority)?;
                match refused {
                    Err(error)
                        if matches!(
                            error.downcast_ref::<IndexError>(),
                            Some(IndexError::SnapshotUnavailable)
                        ) && missing_nonce => {}
                    Err(error)
                        if matches!(
                            error.downcast_ref::<IndexError>(),
                            Some(IndexError::DigestConflict)
                        ) && !missing_nonce => {}
                    Err(error) => return Err(error.context("wrong Source/Q incarnation refusal")),
                    Ok(()) => accepted_nonce[case] = true,
                }
            }
            if accepted_nonce.iter().any(|accepted| *accepted) {
                return Err(anyhow!(
                    "Source candidate nonce authenticated the actual Q publication: missing={}, foreign={}",
                    accepted_nonce[0],
                    accepted_nonce[1]
                ));
            }
            source.baseline_candidate_root_nonce = Some(Digest(snapshot.root_nonce));
            let wrong = boot::RecoverySourceInputs {
                current_target: &full_target,
                current_request: &fixture.captured,
                ..source
            };
            let capture = boot::RecoveryCapture {
                source: &wrong,
                full: &full,
            };
            let refused = boot::authenticate(
                &authority,
                snapshot,
                selected,
                &registry,
                fixture.config.resources.physical.inventory.max_entries,
                Some(&capture),
                budget,
            );
            boot::require_retired(&authority)?;
            match refused {
                Err(error)
                    if matches!(
                        error.downcast_ref::<IndexError>(),
                        Some(IndexError::DigestConflict)
                    ) =>
                {
                    Ok(())
                }
                Err(error) => Err(error.context("wrong historical FULL/current DELTA refusal")),
                Ok(()) => Err(anyhow!(
                    "historical FULL was accepted as current DELTA evidence"
                )),
            }
        })
    })?;
    Ok(())
}

#[test]
fn actual_stale_catalog_without_ast_refuses_missing_current_native_row() -> TestResult {
    let mut fixture = prepare_native_fixture()?;
    fixture.running.stop()?;
    let registry = compose_boot_registry(&fixture.config)?;
    let select = NativeStateCommandV1 {
        mode: NativeCommandModeV1::Select,
        config_path: fixture.config_path.clone(),
        identity: None,
        config_digest: Some(fixture.config_digest),
        install_catalog_digest: None,
        select_operation: Some(operation(2)),
        receipt_path: Some(fixture.receipt_path.clone()),
    };
    require(
        boot::execute(&select, &fixture.config, &registry, request())?.is_none(),
        "actual FULL BOOT selection returned a live owner",
    )?;
    let catalog = quanta_index_catalog::catalog_dir(&fixture.root)
        .join(quanta_index_catalog::CATALOG_FILE_NAME);
    let names = [
        catalog.clone(),
        catalog.with_file_name(format!("{}-wal", quanta_index_catalog::CATALOG_FILE_NAME)),
        catalog.with_file_name(format!("{}-shm", quanta_index_catalog::CATALOG_FILE_NAME)),
    ];
    // Fault-fixture setup only: freeze real quiescent Q bytes BEFORE N. This
    // is not an admitted backup API or a fabricated signed Source artifact.
    let saved = {
        let held =
            quanta_index_searchd::app::runtime::StateRootLease::acquire_existing(&fixture.root)?;
        let mut saved = Vec::new();
        for name in &names {
            let bytes = match fs::read(name) {
                Ok(bytes) => Some(bytes),
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
                Err(error) => return Err(error.into()),
            };
            saved.push(bytes);
        }
        held.require_state_root_v1(&fixture.root)?;
        saved
    };
    let service = bind(
        &fixture.config,
        compose(&fixture.config)?,
        None,
        &request(),
        false,
    )?;
    fixture.running = RunningFixture {
        shutdown: service.shutdown_handle(),
        thread: Some(
            thread::Builder::new()
                .name("stale-catalog-native".into())
                .spawn(move || service.run(Duration::from_millis(10)))?,
        ),
    };
    let (_current_view, _record, current_request) = advance_source_with_capture(&fixture)?;
    let UpdateKind::SourceAdvance(current_transition) = &current_request.kind else {
        return Err("actual current request is not SourceAdvance".into());
    };
    let expected_source = current_transition.expected_source;
    let full_target = NativeCutoverEvidenceTargetV1 {
        operation: operation(2),
        expected_view: fixture.configured,
        expected_source: fixture.prior.source,
        expected_profile: fixture.profile,
        source_mode: SourceOperationMode::Advance,
        event_predecessor: 0,
        event_sequence: 1,
        text_admission: SourceTextAdmission::Utf8 {
            policy: Digest([31; 32]),
        },
        owner_identity_policy_digest: Digest([0; 32]),
        canonical_target_digest: Digest([0; 32]),
    };
    let current_target = NativeCutoverEvidenceTargetV1 {
        operation: operation(3),
        expected_view: fixture.native_view,
        expected_source,
        event_predecessor: 1,
        event_sequence: 2,
        ..full_target
    };
    fixture.running.stop()?;
    {
        let held =
            quanta_index_searchd::app::runtime::StateRootLease::acquire_existing(&fixture.root)?;
        for (name, bytes) in names.iter().zip(saved) {
            if let Some(bytes) = bytes {
                fs::write(name, bytes)?;
            } else {
                match fs::remove_file(name) {
                    Ok(()) => {}
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                    Err(error) => return Err(error.into()),
                }
            }
        }
        held.require_state_root_v1(&fixture.root)?;
    }
    boot::with_admitted_control_budget(&fixture.config, request(), false, |budget, _cache| {
        boot::with_boot_authority(&fixture.config, budget, |authority, budget| {
            // Old FULL and the same installed BOOT still exist. N's actual
            // row is gone. There is no AST to accidentally reject this shape.
            // Prove this is the intended coherent old FULL/BOOT, before the
            // missing-current-row refusal gate. A missing/corrupt baseline or
            // an authority/setup failure must not count as this negative.
            let baseline = quanta_index_catalog::verify_native_boot_catalog(
                &fixture.root,
                authority.io.as_ref(),
                budget,
            )?;
            require(
                baseline
                    .selected
                    .as_ref()
                    .is_some_and(|selected| selected.operation.key == operation(2)),
                "stale fixture did not retain the actual FULL BOOT baseline",
            )
            .map_err(|error| anyhow!(error.to_string()))?;
            drop(baseline);
            let full_result = quanta_index_catalog::verify_native_rollback_with_source_publications(
                &fixture.root,
                authority.io.as_ref(),
                operation(2),
                operation(3),
                budget,
            );
            // The canonical 2-key reader now refuses before authenticate.
            // Its actual FD/close retirement must succeed even on this refusal.
            boot::require_retired(&authority)?;
            let full = match full_result {
                Err(IndexError::SnapshotUnavailable) => return Ok(()),
                Err(error) => return Err(error.into()),
                Ok(full) => full,
            };
            let snapshot = full.snapshot();
            let selected = snapshot
                .selected
                .as_ref()
                .ok_or(IndexError::SnapshotUnavailable)?;
            require(
                selected.operation.key == operation(2),
                "stale fixture did not restore the real FULL Catalog state",
            )
            .map_err(|error| anyhow!(error.to_string()))?;
            let source = boot::RecoverySourceInputs {
                target: &full_target,
                baseline_candidate_root_nonce: Some(Digest(snapshot.root_nonce)),
                request: &fixture.captured,
                current_target: &current_target,
                current_request: &current_request,
                legacy_publication: None,
                joined_source: None,
                ast_work: None,
            };
            let capture = boot::RecoveryCapture {
                source: &source,
                full: &full,
            };
            let refused = boot::authenticate(
                &authority,
                snapshot,
                selected,
                &registry,
                fixture.config.resources.physical.inventory.max_entries,
                Some(&capture),
                budget,
            );
            boot::require_retired(&authority)?;
            match refused {
                Err(error)
                    if matches!(
                        error.downcast_ref::<IndexError>(),
                        Some(IndexError::SnapshotUnavailable)
                    ) =>
                {
                    Ok(())
                }
                Err(error) => Err(error.context("wrong stale current Native metadata refusal")),
                Ok(()) => Err(anyhow!(
                    "stale FULL Catalog was accepted as the current Native observation"
                )),
            }
        })
    })?;
    Ok(())
}

#[test]
fn actual_selected_ast_after_delta_uses_current_native_same_snapshot_publication() -> TestResult {
    let fixture = prepare_native_fixture()?;
    let (current_view, record, current_request) = advance_source_with_capture(&fixture)?;
    let UpdateKind::SourceAdvance(current_transition) = &current_request.kind else {
        return Err("actual current request is not SourceAdvance".into());
    };
    let expected_source = current_transition.expected_source;
    let full_target = NativeCutoverEvidenceTargetV1 {
        operation: operation(2),
        expected_view: fixture.configured,
        expected_source: fixture.prior.source,
        expected_profile: fixture.profile,
        source_mode: SourceOperationMode::Advance,
        event_predecessor: 0,
        event_sequence: 1,
        text_admission: SourceTextAdmission::Utf8 {
            policy: Digest([31; 32]),
        },
        // Q-only captured request fixture; no signed producer permission.
        owner_identity_policy_digest: Digest([0; 32]),
        canonical_target_digest: Digest([0; 32]),
    };
    let current_target = NativeCutoverEvidenceTargetV1 {
        operation: operation(3),
        expected_view: fixture.native_view,
        expected_source,
        event_predecessor: 1,
        event_sequence: 2,
        ..full_target
    };
    let current = fixture
        .client
        .read_view(ViewSelection::Exact(current_view), &request())?;
    let (ast_request, ast_view) = publish_ast(&fixture, &current, record)?;
    let projected = fixture
        .client
        .read_view(ViewSelection::Exact(ast_view), &request())?;
    require(
        projected.source == current.source,
        "actual AST changed current SourceCut",
    )?;
    require(
        ast_view != current_view,
        "actual AST did not advance the DELTA view",
    )?;
    fixture.running.finish()?;
    let registry = compose_boot_registry(&fixture.config)?;
    let select = NativeStateCommandV1 {
        mode: NativeCommandModeV1::Select,
        config_path: fixture.config_path.clone(),
        identity: None,
        config_digest: Some(fixture.config_digest),
        install_catalog_digest: None,
        select_operation: Some(ast_request.operation),
        receipt_path: Some(fixture.receipt_path.clone()),
    };
    require(
        boot::execute(&select, &fixture.config, &registry, request())?.is_none(),
        "actual AST-after-DELTA BOOT selection returned a live owner",
    )?;
    boot::with_admitted_control_budget(&fixture.config, request(), false, |budget, _cache| {
        boot::with_boot_authority(&fixture.config, budget, |authority, budget| {
            let full = quanta_index_catalog::verify_native_rollback_with_source_publications(
                &fixture.root,
                authority.io.as_ref(),
                operation(2),
                operation(3),
                budget,
            )?;
            let (delta_operation, delta_publication) = full
                .published_for(operation(3))
                .ok_or(IndexError::SnapshotUnavailable)?;
            require(
                delta_operation.key == operation(3)
                    && delta_publication.identity == current_view
                    && delta_publication.source == current.source,
                "same-snapshot current DELTA loan differs",
            )
            .map_err(|error| anyhow!(error.to_string()))?;
            let snapshot = full.snapshot();
            let selected = snapshot
                .selected
                .as_ref()
                .ok_or(IndexError::SnapshotUnavailable)?;
            require(
                selected.operation.key == ast_request.operation
                    && selected.publication.identity == ast_view,
                "actual selected AST-after-DELTA metadata differs",
            )
            .map_err(|error| anyhow!(error.to_string()))?;
            let source = boot::RecoverySourceInputs {
                target: &full_target,
                baseline_candidate_root_nonce: Some(Digest(snapshot.root_nonce)),
                request: &fixture.captured,
                current_target: &current_target,
                current_request: &current_request,
                legacy_publication: None,
                joined_source: None,
                ast_work: Some(boot::RecoveryAstInputs {
                    operation: ast_request.operation,
                    publication: ast_view,
                    source_view: current_view,
                    source_cut: current.source,
                    request: &ast_request,
                }),
            };
            let capture = boot::RecoveryCapture {
                source: &source,
                full: &full,
            };
            boot::authenticate(
                &authority,
                snapshot,
                selected,
                &registry,
                fixture.config.resources.physical.inventory.max_entries,
                Some(&capture),
                budget,
            )?;
            boot::require_retired(&authority)?;
            let wrong_ast = boot::RecoverySourceInputs {
                ast_work: Some(boot::RecoveryAstInputs {
                    operation: ast_request.operation,
                    publication: ast_view,
                    source_view: fixture.native_view,
                    source_cut: current.source,
                    request: &ast_request,
                }),
                ..source
            };
            let refused = boot::authenticate(
                &authority,
                snapshot,
                selected,
                &registry,
                fixture.config.resources.physical.inventory.max_entries,
                Some(&boot::RecoveryCapture {
                    source: &wrong_ast,
                    full: &full,
                }),
                budget,
            );
            boot::require_retired(&authority)?;
            match refused {
                Err(error)
                    if matches!(
                        error.downcast_ref::<IndexError>(),
                        Some(IndexError::DigestConflict)
                    ) => {}
                Err(error) => return Err(error.context("wrong historical AST source-view refusal")),
                Ok(()) => {
                    return Err(anyhow!(
                        "historical FULL view accepted as AST source view after DELTA"
                    ));
                }
            }
            let wrong = boot::RecoverySourceInputs {
                current_target: &full_target,
                current_request: &fixture.captured,
                ast_work: Some(boot::RecoveryAstInputs {
                    operation: ast_request.operation,
                    publication: ast_view,
                    source_view: current_view,
                    source_cut: current.source,
                    request: &ast_request,
                }),
                ..source
            };
            let capture = boot::RecoveryCapture {
                source: &wrong,
                full: &full,
            };
            let refused = boot::authenticate(
                &authority,
                snapshot,
                selected,
                &registry,
                fixture.config.resources.physical.inventory.max_entries,
                Some(&capture),
                budget,
            );
            boot::require_retired(&authority)?;
            match refused {
                Err(error)
                    if matches!(
                        error.downcast_ref::<IndexError>(),
                        Some(IndexError::DigestConflict)
                    ) =>
                {
                    Ok(())
                }
                Err(error) => {
                    Err(error.context("wrong historical FULL/current DELTA/selected AST refusal"))
                }
                Ok(()) => Err(anyhow!(
                    "historical FULL was accepted as current DELTA for selected AST"
                )),
            }
        })
    })?;
    Ok(())
}
