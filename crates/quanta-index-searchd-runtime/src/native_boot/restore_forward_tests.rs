//! Q component proof using actual UDS FULL publications and Catalog BOOT.
//! No signed Source, old Legacy publication, or selection success is minted.
use super::*;
use quanta_index_searchd::cli::NativeBootIdentityV1;

#[test]
fn restore_forward_retained_terminal_binds_history_presence_and_cut() {
    use quanta_index_contract::index::{AuxiliaryGenerationKeyV1, HistorySourceCutV1};
    use quanta_index_contract::native_cutover_evidence_v1::{
        NativeCutoverControlTerminalV1 as Terminal, NativeCutoverInstalledHistoryV1,
    };
    use quanta_index_contract::{AuxEpochV1, ManifestGeneration, RepoId, RevisionId};

    let history = NativeCutoverInstalledHistoryV1 {
        operation: operation(3),
        view: ViewRef {
            workspace: workspace(),
            epoch: 1,
            sequence: 6,
            digest: Digest([10; 32]),
        },
        instance: IndexInstanceId([11; 16]),
        member: ObjectRef(Digest([12; 32])),
        source_input: ObjectRef(Digest([13; 32])),
        cut: HistorySourceCutV1 {
            generation: AuxiliaryGenerationKeyV1 {
                repo_id: RepoId::new("repo").expect("fixture repository"),
                revision_id: RevisionId::new("revision").expect("fixture revision"),
                generation: ManifestGeneration::new(1),
            },
            durable_epoch: AuxEpochV1::new(2),
            snapshot_digest: Digest([14; 32]),
            replay_start: Digest([15; 32]),
        },
    };
    let before_sha256 = Digest([2; 32]);
    let after_sha256 = Digest([3; 32]);
    let applied = Terminal::Applied {
        operation_id: "restore-forward".to_owned(),
        nonce: Digest([1; 32]),
        before_sha256,
        after_sha256,
    };
    let reconciled = Terminal::ReconciledHistory {
        operation_id: "restore-forward".to_owned(),
        nonce: Digest([4; 32]),
        before_sha256,
        after_sha256,
        history: history.clone(),
    };
    let matches = |retained_terminal, observed| {
        boot::RestoreForwardHistoryV1 {
            retained_terminal,
            selected: &mut None,
        }
        .matches_retained_v1(observed)
    };
    assert!(matches(None, Some(&history)));
    assert!(matches(Some(&applied), None));
    assert!(!matches(Some(&applied), Some(&history)));
    assert!(matches(Some(&reconciled), Some(&history)));
    assert!(!matches(Some(&reconciled), None));

    let mut changed_cut = history.clone();
    changed_cut.cut.replay_start = Digest([16; 32]);
    assert!(!matches(Some(&reconciled), Some(&changed_cut)));
}

fn target(fixture: &PreparedNativeFixture) -> NativeCutoverEvidenceTargetV1 {
    NativeCutoverEvidenceTargetV1 {
        operation: fixture.captured.operation,
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
    }
}

#[test]
#[cfg(target_os = "linux")]
fn recovery_native_root_inventory_spends_original_control_budget() -> TestResult {
    let mut fixture = prepare_native_fixture()?;
    fixture.running.stop()?;
    boot::with_admitted_control_budget(&fixture.config, request(), false, |budget, cache| {
        anyhow::ensure!(cache.is_none(), "recovery reserved a writer cache");
        let before = budget.usage();
        boot::with_recovery_boot_authority(&fixture.config, budget, |authority, budget| {
            let after = budget.usage();
            anyhow::ensure!(
                after.reads > before.reads && after.bytes.saturating_sub(before.bytes) >= 65_536,
                "Native recovery inventory did not charge a directory buffer to Original"
            );
            let snapshot = quanta_index_catalog::verify_native_boot_catalog(
                &fixture.root,
                authority.io.as_ref(),
                budget,
            )?;
            anyhow::ensure!(
                snapshot.selected.is_none(),
                "fixture unexpectedly selected BOOT"
            );
            drop(snapshot);
            boot::require_retired(&authority)?;
            Ok(())
        })
    })?;
    Ok(())
}

#[test]
fn fresh_restore_candidate_requires_exact_boot_and_root_scoped_source() -> TestResult {
    let mut old = prepare_native_fixture()?;
    let old_live = old
        .client
        .observe_root_view(ViewSelection::Exact(old.native_view), &request())?;
    old.running.stop()?;
    let mut fresh = prepare_native_fixture()?;
    let fresh_live = fresh
        .client
        .observe_root_view(ViewSelection::Exact(fresh.native_view), &request())?;
    fresh.running.stop()?;
    require(
        old_live.root_nonce != fresh_live.root_nonce,
        "fixture roots alias",
    )?;
    require(
        old_live.view.source == fresh_live.view.source
            && old_live.view.profile == fresh_live.view.profile,
        "fixture must isolate incarnation from logical Source",
    )?;
    let registry = compose_boot_registry(&fresh.config)?;
    let select = NativeStateCommandV1 {
        mode: NativeCommandModeV1::Select,
        config_path: fresh.config_path.clone(),
        identity: None,
        config_digest: Some(fresh.config_digest),
        install_catalog_digest: None,
        select_operation: Some(fresh.captured.operation),
        receipt_path: Some(fresh.receipt_path.clone()),
    };
    require(
        boot::execute(&select, &fresh.config, &registry, request())
            .map_err(|error| error.context(boot_fixture_failure_context(&fresh.root)))?
            .is_none(),
        "offline Select returned live authority",
    )?;
    let target = target(&fresh);
    boot::with_admitted_control_budget(&fresh.config, request(), false, |budget, _cache| {
        boot::with_boot_authority(&fresh.config, budget, |authority, budget| {
            let snapshot = quanta_index_catalog::verify_native_boot_catalog(
                &fresh.root,
                authority.io.as_ref(),
                budget,
            )?;
            let selected = snapshot
                .selected
                .as_ref()
                .ok_or(IndexError::SnapshotUnavailable)?;
            let identity = boot::identity(snapshot.root_nonce, selected)?;
            drop(snapshot);
            let command = NativeStateCommandV1 {
                mode: NativeCommandModeV1::Verify,
                identity: Some(identity),
                select_operation: None,
                receipt_path: None,
                ..select
            };
            let source = || boot::RecoverySourceInputs {
                target: &target,
                baseline_candidate_root_nonce: Some(fresh_live.root_nonce),
                request: &fresh.captured,
                current_target: &target,
                current_request: &fresh.captured,
                legacy_publication: None,
                joined_source: None,
                ast_work: None,
            };
            let maximum = fresh.config.resources.physical.inventory.max_entries;
            let full = boot::verify_recovery_candidate_with_source_capture_v1(
                &authority,
                &command,
                &registry,
                &source(),
                maximum,
                budget,
            )?;
            anyhow::ensure!(
                full.publication() == fresh_live.view.identity,
                "wrong verified FULL"
            );
            drop(full);
            let foreign = boot::RecoverySourceInputs {
                baseline_candidate_root_nonce: Some(old_live.root_nonce),
                ..source()
            };
            let wrong_profile = NativeCutoverEvidenceTargetV1 {
                expected_profile: ObjectRef(Digest([0xff; 32])),
                ..target
            };
            let wrong_current = boot::RecoverySourceInputs {
                baseline_candidate_root_nonce: Some(fresh_live.root_nonce),
                current_target: &wrong_profile,
                ..source()
            };
            let wrong_boot = NativeStateCommandV1 {
                identity: Some(NativeBootIdentityV1 {
                    root_nonce: old_live.root_nonce.0,
                    ..identity
                }),
                ..command.clone()
            };
            let correct = source();
            for (candidate, projection) in [
                (&command, &foreign),
                (&command, &wrong_current),
                (&wrong_boot, &correct),
            ] {
                let observed = boot::verify_recovery_candidate_with_source_capture_v1(
                    &authority, candidate, &registry, projection, maximum, budget,
                );
                anyhow::ensure!(
                    matches!(observed, Err(error) if error.downcast_ref::<IndexError>() == Some(&IndexError::DigestConflict)),
                    "candidate mismatch did not reach the typed Q identity check",
                );
                boot::require_retired(&authority)?;
            }
            Ok(())
        })
    })?;
    Ok(())
}

#[cfg(target_os = "linux")]
fn selected_native_wire_v1(fixture: &PreparedNativeFixture) -> TestResult<Vec<u8>> {
    let receipt: serde_json::Value = serde_json::from_slice(&fs::read(&fixture.receipt_path)?)?;
    let mut bytes = serde_json::to_vec(&serde_json::json!({
        "schema_version": 2,
        "mode": "native",
        "binary": fixture.binary,
        "binary_sha256": hex(fixture.binary_digest),
        "state_root": fixture.root,
        "environment": {},
        "native_config": fixture.config_path,
        "native_config_sha256": hex(fixture.config_digest),
        "native_boot_receipt": receipt,
    }))?;
    bytes.push(b'\n');
    Ok(bytes)
}

#[cfg(target_os = "linux")]
fn select_native_fixture_v1(fixture: &PreparedNativeFixture) -> TestResult {
    let registry = compose_boot_registry(&fixture.config)?;
    let command = NativeStateCommandV1 {
        mode: NativeCommandModeV1::Select,
        config_path: fixture.config_path.clone(),
        identity: None,
        config_digest: Some(fixture.config_digest),
        install_catalog_digest: None,
        select_operation: Some(fixture.captured.operation),
        receipt_path: Some(fixture.receipt_path.clone()),
    };
    require(
        boot::execute(&command, &fixture.config, &registry, request())?.is_none(),
        "offline Select unexpectedly returned a live authority",
    )
}

#[cfg(target_os = "linux")]
fn probe_previous_native_v1(
    current: &PreparedNativeFixture,
    candidate: &PreparedNativeFixture,
    current_wire: &[u8],
    check: impl FnOnce(
        &crate::deployment_selection::DeploymentSelectionDocumentV1,
        &mut WorkBudget,
    ) -> TestResult,
) -> TestResult {
    let selection = current.base.join("previous-native-selection.json");
    let next = current.base.join("fresh-native-candidate.json");
    let candidate_wire = selected_native_wire_v1(candidate)?;
    for (path, wire) in [
        (&selection, current_wire),
        (&next, candidate_wire.as_slice()),
    ] {
        fs::write(path, wire)?;
        fs::set_permissions(path, fs::Permissions::from_mode(0o600))?;
    }
    let before = fs::metadata(&selection)?;
    let request_v1 = DeploymentSelectionRequestV1 {
        selection_path_v1: &selection,
        expected_current_sha256_v1: Digest(Sha256::digest(current_wire).into()),
        candidate_path_v1: &next,
        expected_candidate_sha256_v1: Digest(Sha256::digest(&candidate_wire).into()),
        mode_v1: DeploymentRecoveryModeV1::RestoreForward,
    };
    let mut observed = None;
    let attempt =
        boot::with_admitted_control_budget(&candidate.config, request(), false, |budget, _| {
            Ok(with_locked_deployment_selection_v1(
                &request_v1,
                budget,
                |locked, budget| {
                    observed = Some(check(locked.current_v1(), budget));
                    Err(DeploymentSelectionErrorV1::RefusedBeforeRename(anyhow!(
                        "observation-only previous Native fixture"
                    )))
                },
            ))
        })?;
    require(
        matches!(
            attempt,
            Err(DeploymentSelectionErrorV1::RefusedBeforeRename(_))
        ),
        "observation-only fixture reached publication",
    )?;
    observed.ok_or("previous Native observation was not entered")??;
    let after = fs::metadata(&selection)?;
    require(
        fs::read(&selection)? == current_wire
            && (before.dev(), before.ino()) == (after.dev(), after.ino()),
        "previous Native observation changed selected bytes or inode",
    )
}

#[test]
#[cfg(target_os = "linux")]
fn held_previous_native_reads_selected_boot_and_refuses_stale_identity_v1() -> TestResult {
    let mut old = prepare_native_fixture()?;
    let mut fresh = prepare_native_fixture()?;
    let old_live = old
        .client
        .observe_root_view(ViewSelection::Exact(old.native_view), &request())?;
    old.running.stop()?;
    fresh.running.stop()?;
    select_native_fixture_v1(&old)?;
    select_native_fixture_v1(&fresh)?;
    let correct = selected_native_wire_v1(&old)?;
    probe_previous_native_v1(&old, &fresh, &correct, |selected, budget| {
        let held = boot::restore_forward::HeldPreviousNativeV1::acquire_v1(selected, budget)?;
        require(
            Digest(held.observed_v1().snapshot.root_nonce) == old_live.root_nonce
                && held.observed_v1().current_head == old_live.view,
            "previous Native nonce or actual Catalog head differs from live Q",
        )?;
        let mut unexpected_history = false;
        let count = held.visit_history_members_v1(None, budget, |_, _, _, _| {
            unexpected_history = true;
            Ok(())
        })?;
        require(
            count == 0 && !unexpected_history,
            "Source-only previous Native root exposed a History member",
        )?;
        held.require_current_and_retired_v1(budget)?;
        let config_bytes = fs::read(&old.config_path)?;
        fs::write(&old.config_path, [config_bytes.as_slice(), b" "].concat())?;
        let stale = held.require_current_and_retired_v1(budget);
        fs::write(&old.config_path, config_bytes)?;
        require(
            stale.is_err(),
            "changed selected config passed pinned digest",
        )?;
        let moved = old.root.with_extension("held-original");
        fs::rename(&old.root, &moved)?;
        fs::create_dir(&old.root)?;
        fs::set_permissions(&old.root, fs::Permissions::from_mode(0o700))?;
        let replaced = held.require_current_and_retired_v1(budget);
        fs::remove_dir(&old.root)?;
        fs::rename(&moved, &old.root)?;
        require(replaced.is_err(), "replaced named root passed held lease")
    })?;
    let mut wrong: serde_json::Value = serde_json::from_slice(&correct)?;
    let mut nonce = wrong["native_boot_receipt"]["root_nonce_hex"]
        .as_str()
        .ok_or("selected BOOT nonce absent")?
        .as_bytes()
        .to_vec();
    nonce[0] = if nonce[0] == b'0' { b'1' } else { b'0' };
    wrong["native_boot_receipt"]["root_nonce_hex"] = serde_json::json!(String::from_utf8(nonce)?);
    let mut wrong = serde_json::to_vec(&wrong)?;
    wrong.push(b'\n');
    probe_previous_native_v1(&old, &fresh, &wrong, |selected, budget| {
        require(
            boot::restore_forward::HeldPreviousNativeV1::acquire_v1(selected, budget).is_err(),
            "foreign selected BOOT nonce authenticated actual root",
        )
    })?;
    let service = bind(&old.config, compose(&old.config)?, None, &request(), false)?;
    let shutdown = service.shutdown_handle();
    let running = thread::Builder::new()
        .name("previous-native-pending".into())
        .spawn(move || service.run(Duration::from_millis(10)))?;
    let _pending = old.client.begin_input(operation(3), &request())?;
    shutdown.trigger();
    running
        .join()
        .map_err(|_| "pending Native service panicked")??;
    probe_previous_native_v1(&old, &fresh, &correct, |selected, budget| {
        require(
            boot::restore_forward::HeldPreviousNativeV1::acquire_v1(selected, budget).is_err(),
            "unsettled previous Native ingress passed held-root quiescence",
        )
    })
}
