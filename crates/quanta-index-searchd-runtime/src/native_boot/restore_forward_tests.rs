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
    select_native_fixture_with_registry_v1(fixture, compose_boot_registry(&fixture.config)?)
}

#[cfg(target_os = "linux")]
fn select_native_fixture_with_registry_v1(
    fixture: &PreparedNativeFixture,
    registry: AdapterRegistry,
) -> TestResult {
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
        &mut quanta_index_core::index::WorkBudget,
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
    let service = bind(
        &old.config,
        old.config_digest,
        compose(&old.config)?,
        None,
        &request(),
        false,
    )?;
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

#[test]
#[cfg(target_os = "linux")]
fn retained_v20_tree_boot_supports_selected_serve_but_not_a_current_candidate() -> TestResult {
    let mut old = prepare_native_fixture_with_tree(true, true)?;
    let mut fresh = prepare_native_fixture_with_tree(false, true)?;
    old.running.stop()?;
    fresh.running.stop()?;
    select_native_fixture_with_registry_v1(&old, retained_tree_v20_registry(&old.config)?)?;
    select_native_fixture_v1(&fresh)?;
    let old_wire = selected_native_wire_v1(&old)?;
    probe_previous_native_v1(&old, &fresh, &old_wire, |selected, budget| {
        let held = boot::restore_forward::HeldPreviousNativeV1::acquire_v1(selected, budget)?;
        held.require_current_and_retired_v1(budget)?;
        Ok(())
    })?;
    let current_registry = compose_boot_registry(&old.config)?;
    boot::with_admitted_control_budget(&old.config, request(), false, |budget, _| {
        boot::with_boot_authority(&old.config, budget, |authority, budget| {
            let snapshot = quanta_index_catalog::verify_native_boot_catalog(
                &old.root,
                authority.io.as_ref(),
                budget,
            )?;
            let selected = snapshot
                .selected
                .as_ref()
                .ok_or(IndexError::SnapshotUnavailable)?;
            for candidate in [false, true] {
                let strict = boot::authenticate(
                    &authority,
                    &snapshot,
                    selected,
                    &current_registry,
                    old.config.resources.physical.inventory.max_entries,
                    None,
                    false,
                    candidate,
                    budget,
                );
                anyhow::ensure!(strict.is_err(), "old Tree policy passed current BOOT");
            }
            let previous = old
                .config
                .text_recipe
                .retained_source_tree_policy_v20()
                .ok_or(IndexError::SnapshotUnavailable)?;
            let _live = boot::authenticate_with_legacy_history_v1(
                &authority,
                &snapshot,
                selected,
                &current_registry,
                old.config.resources.physical.inventory.max_entries,
                None,
                None,
                None,
                boot::ProtectedRouteCheckV1::Retained,
                true,
                Some(quanta_index_kernel::RetainedSourceTreePolicyV20 {
                    descriptor: old.config.text,
                    previous,
                    current: old.config.text_recipe.output_policy(),
                }),
                budget,
            )?;
            let wrong_history = boot::authenticate_retained_source_v20(
                &authority,
                &snapshot,
                selected,
                &current_registry,
                old.config.resources.physical.inventory.max_entries,
                quanta_index_kernel::RetainedSourceTreePolicyV20 {
                    descriptor: old.config.text,
                    previous: Digest([0; 32]),
                    current: old.config.text_recipe.output_policy(),
                },
                budget,
            );
            anyhow::ensure!(
                wrong_history.is_err(),
                "foreign Tree recipe passed retained BOOT"
            );
            Ok(())
        })
    })?;
    Ok(())
}

#[test]
#[cfg(target_os = "linux")]
fn cold_settlement_rechecks_not_applicable_on_the_original_legacy_scope() -> TestResult {
    use crate::deployment_selection::{
        DeploymentSelectionBeforeDocumentV1, LegacyPublishedRecoveryWitnessV1,
        PublishedRecoveryRootWitnessV1, with_locked_deployment_selection_reconciliation_v1,
        with_locked_published_recovery_v1,
    };
    use quanta_index_contract::{
        FileId, ManifestGeneration, RepoId, RepoMapActivateGenerationRequestV2,
        RepoMapExactnessSummary, RepoMapFileNode, RepoMapGraphCoverage, RepoMapGraphCoverageClass,
        RepoMapItemIndexAvailability, RepoMapNode, RepoMapPublishBundleRequestV2,
        RepoMapRedactionState, RepoMapSourceBundle, RepoRelativePath, RevisionId,
        native_cutover_evidence_v1::{
            NativeCutoverControlBeginV1, NativeCutoverDigest64V1,
            NativeCutoverEvidenceLegacyRepoMapV1, NativeCutoverRecoveryModeV1,
        },
    };
    use quanta_index_core::publication_capacity::InodeKey;
    use quanta_index_searchd_harness::legacy_reference::{
        LegacySearchdBinary, publish_legacy_source,
    };
    use std::{collections::BTreeMap, sync::Arc};

    for add_active_repomap_after_publication in [false, true] {
        let mut native = prepare_native_fixture()?;
        native.running.stop()?;
        select_native_fixture_v1(&native)?;

        let old_root = native.base.join("original-legacy");
        let old_binary = LegacySearchdBinary::from_environment()?;
        let old_process = old_binary.start(&old_root, 8)?;
        let old_client = old_process.connect()?;
        let repo = RepoId::new("original-repo")?;
        let revision = RevisionId::new("original-revision")?;
        let (head, source_receipt) = publish_legacy_source(
            &old_client,
            &repo,
            &revision,
            1,
            None,
            &[("src/original.rs", "fn original() {}", 1)],
            None,
        )?;
        drop(old_client);
        old_process.stop()?;
        old_binary.verify()?;

        let installed_old_binary = old_binary.installer_binary()?;
        let old_binary_path = installed_old_binary.path().to_path_buf();
        installed_old_binary.verify()?;
        let old_binary_sha256 = hex(sha_file(&old_binary_path)?);
        let environment =
            legacy_physical_fixture::legacy_physical_env().collect::<BTreeMap<_, _>>();
        let mut old_selection = serde_json::json!({
            "schema_version": 2,
            "mode": "legacy",
            "binary": old_binary_path,
            "binary_sha256": old_binary_sha256,
            "state_root": old_root,
            "environment": environment,
        });
        old_selection.sort_all_objects();
        let mut before_bytes = serde_json::to_vec(&old_selection)?;
        before_bytes.push(b'\n');
        let after_bytes = selected_native_wire_v1(&native)?;
        let selection = native.base.join("cold-selection.json");
        let candidate = native.base.join("cold-candidate.json");
        for (path, bytes) in [
            (&selection, before_bytes.as_slice()),
            (&candidate, after_bytes.as_slice()),
        ] {
            fs::write(path, bytes)?;
            fs::set_permissions(path, fs::Permissions::from_mode(0o600))?;
        }
        let before_inode = fs::metadata(&selection)?;
        let before_sha256 = Digest(Sha256::digest(&before_bytes).into());
        let after_sha256 = Digest(Sha256::digest(&after_bytes).into());
        let request_v1 = DeploymentSelectionRequestV1 {
            selection_path_v1: &selection,
            expected_current_sha256_v1: before_sha256,
            candidate_path_v1: &candidate,
            expected_candidate_sha256_v1: after_sha256,
            mode_v1: DeploymentRecoveryModeV1::RestoreForward,
        };
        let before = DeploymentSelectionBeforeDocumentV1 {
            operation_id_v1: "original-not-applicable-cold-settlement",
            bytes_v1: &before_bytes,
            raw_sha256_v1: before_sha256,
            file_version_v1: Some(
                quanta_index_contract::native_cutover_evidence_v1::NativeCutoverEvidenceBeforeSelectionV1::file_version_v1(&before_inode),
            ),
            retained_terminal_v1: None,
        };
        let begin = NativeCutoverControlBeginV1 {
            format_version: 1,
            operation_id: before.operation_id_v1.to_owned(),
            mode: NativeCutoverRecoveryModeV1::RestoreForward,
            current_selection_sha256: before_sha256,
            candidate_selection_sha256: after_sha256,
            deadline_ns: 1,
            evidence_length: 1,
            evidence_sha256: Digest([9; 32]),
            nonce: Digest([10; 32]),
        };
        let disposition = NativeCutoverEvidenceLegacyRepoMapV1::NotApplicable {
            reason_code: "no-repomap".into(),
            policy_digest: NativeCutoverDigest64V1([b'a'; 64]),
            product_scope_digest: NativeCutoverDigest64V1([b'b'; 64]),
        };
        let root_metadata = fs::metadata(&native.root)?;
        let native_root = InodeKey {
            device: root_metadata.dev(),
            inode: root_metadata.ino(),
        };
        let published =
            boot::with_admitted_control_budget(&native.config, request(), false, |budget, _| {
                Ok(with_locked_deployment_selection_reconciliation_v1(
                    &request_v1,
                    &before,
                    budget,
                    |locked, budget| {
                        let legacy = LegacyPublishedRecoveryWitnessV1 {
                            repo_id: &repo,
                            revision_id: &revision,
                            durable_sequence: source_receipt.durable_sequence,
                            activation:
                                crate::deployment_selection::legacy_recovery_activation_digest_v1(
                                    &head, budget,
                                )
                                .map_err(DeploymentSelectionErrorV1::RefusedBeforeRename)?,
                        };
                        locked
                            .bind_recovery_witness_v1(
                                &begin,
                                native.config_digest,
                                native.binary_digest,
                            )
                            .and_then(|()| {
                                locked.prepare_recovery_witness_v1(
                                    None,
                                    native_root,
                                    Some(legacy),
                                    &disposition,
                                    budget,
                                )
                            })
                            .map_err(DeploymentSelectionErrorV1::RefusedBeforeRename)?;
                        locked.publish_with_custody_v1(budget, &mut |_| Ok(()))
                    },
                ))
            })??;
        require(
            published.after_sha256_v1() == after_sha256,
            "fixture did not publish the selected Native root",
        )?;
        let selected_inode = fs::metadata(&selection)?;
        require(
            fs::read(&selection)? == after_bytes && selected_inode.ino() != before_inode.ino(),
            "fixture did not retain the actual Native selection inode",
        )?;

        if add_active_repomap_after_publication {
            let catalog = Arc::new(quanta_index_catalog::SqliteCatalog::open(
                &old_root,
                Duration::ZERO,
            )?);
            let store = quanta_index_repomap::RepoMapGenerationStore::open(
                old_root.join("repo-map"),
                catalog,
            )?
            .store;
            let bundle = RepoMapSourceBundle::new(
                repo.clone(),
                revision.clone(),
                ManifestGeneration::new(2),
                "a".repeat(64),
                "original-snapshot",
                1,
                "b".repeat(64),
                RepoMapGraphCoverage {
                    item_index_availability: RepoMapItemIndexAvailability::Available,
                    graph_coverage_class: RepoMapGraphCoverageClass::Complete,
                },
                RepoMapExactnessSummary::Exact,
                RepoMapRedactionState::Unredacted,
            )
            .with_node(RepoMapNode::File(RepoMapFileNode {
                file_id: FileId::new("file://src/original.rs"),
                repo_relative_path: RepoRelativePath::new("src/original.rs"),
                line_count: 1,
            }));
            let _publish =
                store.ingest_bundle_v2(&RepoMapPublishBundleRequestV2::new(bundle.clone())?)?;
            let _activate = store.activate_generation_v2(
                &RepoMapActivateGenerationRequestV2::for_bundle(&bundle)?,
            )?;
        }

        let mut sync_entered = false;
        let outcome =
            boot::with_admitted_control_budget(&native.config, request(), false, |budget, _| {
                Ok(with_locked_published_recovery_v1(
                    &request_v1,
                    &before,
                    native.config_digest,
                    budget,
                    |locked, _, witness: PublishedRecoveryRootWitnessV1<'_>, budget, sync| {
                        boot::with_verified_settlement_root_v1(
                            locked,
                            &native.config,
                            witness,
                            budget,
                            |budget| {
                                sync_entered = true;
                                sync(budget, &mut |_| Ok(()))
                            },
                        )
                    },
                ))
            })?;
        if add_active_repomap_after_publication {
            require(
                matches!(outcome, Err(DeploymentSelectionErrorV1::PersistenceUncertain(error))
                    if error.downcast_ref::<IndexError>() == Some(&IndexError::DigestConflict))
                    && !sync_entered,
                "cold NotApplicable settlement accepted the original scope's active RepoMap",
            )?;
        } else {
            require(
                outcome?.publication.resolution_v1()
                    == crate::deployment_selection::DeploymentSelectionResolutionV1::Reconciled
                    && sync_entered,
                "cold NotApplicable settlement did not reconcile an empty RepoMap scope",
            )?;
        }
        let after_settlement = fs::metadata(&selection)?;
        require(
            fs::read(&selection)? == after_bytes
                && (after_settlement.dev(), after_settlement.ino())
                    == (selected_inode.dev(), selected_inode.ino()),
            "cold settlement changed the selected Native bytes or inode",
        )?;
    }
    Ok(())
}
