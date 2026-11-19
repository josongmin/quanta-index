//! Offline physical proof. The helper does not receive a simulated Source permit.
use super::*;
use crate::native::NativeBootAuthority;
use quanta_index_contract::{
    FileId, ManifestGeneration, RepoId, RepoMapActivateGenerationRequestV2,
    RepoMapExactnessSummary, RepoMapFileNode, RepoMapGraphCoverage, RepoMapGraphCoverageClass,
    RepoMapItemIndexAvailability, RepoMapNode, RepoMapPublishBundleRequestV2,
    RepoMapRedactionState, RepoMapSourceBundle, RepoRelativePath, RevisionId,
    native_cutover_evidence_v1::{NativeCutoverDigest64V1, NativeCutoverEvidenceLegacyRepoMapV1},
    product_root_locator_v2::{
        PRODUCT_ROOT_IDENTITY_FILE_V2, ProductRootKindV2, decode_product_root_locator_v2,
        encode_product_root_identity_v2,
    },
};
use quanta_index_core::index::{WorkBudget, existing_file::ExistingFileReadFailureV1};

#[test]
#[cfg_attr(
    not(target_os = "linux"),
    ignore = "admitted recovery inventory uses Linux getdents64"
)]
fn legacy_active_repomap_recovery_holds_a_separate_real_root() -> TestResult {
    let fixture = prepare_native_fixture()?;
    fixture.running.finish()?;
    let native = &fixture.root;
    let search = &fixture.legacy;
    let repomap = fixture.base.join("repomap");
    let repo = RepoId::new("separate-root")?;
    let revision = RevisionId::new("current-revision")?;
    let (publish, activation, marker, locator) = {
        let lease = quanta_index_searchd::app::runtime::StateRootLease::acquire(&repomap)?;
        let inode = lease.observe_current_root_inode_v1()?;
        let marker = encode_product_root_identity_v2(ProductRootKindV2::RepoMap, [11; 32]);
        fs::write(repomap.join(PRODUCT_ROOT_IDENTITY_FILE_V2), marker)?;
        let locator = decode_product_root_locator_v2(
            &marker,
            ProductRootKindV2::RepoMap,
            inode.device,
            inode.inode,
        )
        .ok_or("fixture marker encoding")?;
        let catalog = Arc::new(quanta_index_catalog::SqliteCatalog::open(
            &repomap,
            Duration::ZERO,
        )?);
        let store =
            quanta_index_repomap::RepoMapGenerationStore::open(repomap.join("repo-map"), catalog)?
                .store;
        let bundle = RepoMapSourceBundle::new(
            repo.clone(),
            revision.clone(),
            ManifestGeneration::new(90),
            "a".repeat(64),
            "snapshot-90",
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
            file_id: FileId::new("file://src/lib.rs"),
            repo_relative_path: RepoRelativePath::new("src/lib.rs"),
            line_count: 1,
        }));
        let publish =
            store.ingest_bundle_v2(&RepoMapPublishBundleRequestV2::new(bundle.clone())?)?;
        let activate = store
            .activate_generation_v2(&RepoMapActivateGenerationRequestV2::for_bundle(&bundle)?)?;
        (publish, activate, marker, NativeCutoverDigest64V1(locator))
    };
    {
        let _lease = quanta_index_searchd::app::runtime::StateRootLease::acquire(search)?;
        drop(quanta_index_catalog::SqliteCatalog::open(
            search,
            Duration::ZERO,
        )?);
    }
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
        "offline Select returned live authority",
    )?;
    let receipt: serde_json::Value = serde_json::from_slice(&fs::read(&fixture.receipt_path)?)?;
    let environment = legacy_physical_fixture::legacy_physical_env()
        .collect::<std::collections::BTreeMap<_, _>>();
    let legacy_document = |path: &Path| {
        serde_json::json!({
            "schema_version": 2, "mode": "legacy", "binary": fixture.binary,
            "binary_sha256": hex(fixture.binary_digest), "state_root": path,
            "environment": environment,
        })
    };
    let write = |path: &Path, document: &serde_json::Value| -> TestResult<Vec<u8>> {
        let mut canonical = document.clone();
        canonical.sort_all_objects();
        let mut bytes = serde_json::to_vec(&canonical)?;
        bytes.push(b'\n');
        fs::write(path, &bytes)?;
        fs::set_permissions(path, fs::Permissions::from_mode(0o600))?;
        Ok(bytes)
    };
    let saved = fixture.base.join("repomap.json");
    let saved_sha256 = Digest(Sha256::digest(write(&saved, &legacy_document(&repomap))?).into());
    let mut cfg: serde_json::Value = serde_json::from_slice(&fs::read(&fixture.config_path)?)?;
    cfg["legacy_repomap_selection"] = serde_json::json!({"path": saved, "sha256": saved_sha256.0});
    let config_sha256 = Digest(Sha256::digest(write(&fixture.config_path, &cfg)?).into());
    let config = read_config_checked(&fixture.config_path, Some(config_sha256))?;
    let selection = fixture.base.join("selection.json");
    let candidate = fixture.base.join("legacy.json");
    let before = write(
        &selection,
        &serde_json::json!({
            "schema_version": 2, "mode": "native", "binary": fixture.binary, "binary_sha256": hex(fixture.binary_digest),
            "state_root": native, "environment": {}, "native_config": fixture.config_path,
            "native_config_sha256": hex(config_sha256), "native_boot_receipt": receipt,
        }),
    )?;
    let after = write(&candidate, &legacy_document(search))?;
    let selection_request = DeploymentSelectionRequestV1 {
        selection_path_v1: &selection,
        candidate_path_v1: &candidate,
        expected_current_sha256_v1: Digest(Sha256::digest(&before).into()),
        expected_candidate_sha256_v1: Digest(Sha256::digest(&after).into()),
        mode_v1: DeploymentRecoveryModeV1::RollbackLegacy,
    };
    let repomap_path = repomap.to_str().ok_or("non-UTF-8 fixture root")?;
    let expected = |activation, locator_digest| NativeCutoverEvidenceLegacyRepoMapV1::Required {
        state_root: repomap_path.into(),
        locator_digest,
        publish: publish.clone(),
        activation,
    };
    let result = boot::with_admitted_control_budget(&config, request(), false, |budget, _| {
        Ok(with_locked_deployment_selection_v1(
            &selection_request,
            budget,
            |locked, budget| {
                let verify_and_refuse =
                    |authority: NativeBootAuthority, budget: &mut WorkBudget| {
                        let (lease, memory) = boot::acquire_recovery_root_v1(search, budget)?;
                        let legacy = boot::legacy_read_factory::LegacyReadAuthorityV1::acquire(
                            locked.candidate_v1(),
                            authority.io.as_ref(),
                            lease,
                            memory,
                            budget,
                        )?;
                        let quiescence = quanta_index_catalog::verify_legacy_catalog_quiescence_v1(
                            search,
                            legacy.catalog(),
                            &repo,
                            &revision,
                            budget,
                        )?;
                        anyhow::ensure!(quiescence.active_repomap_candidate_v1().is_none());
                        let verify =
                            |expected: &NativeCutoverEvidenceLegacyRepoMapV1,
                             budget: &mut WorkBudget| {
                                boot::verify_legacy_repomap_v1(
                                    &legacy,
                                    &quiescence,
                                    expected,
                                    Some(authority.io.as_ref()),
                                    &config,
                                    locked,
                                    budget,
                                )
                            };
                        // The Native→Native receiver reuses this exact saved
                        // selection and physical RepoMap authority. Its
                        // signed Required input must still hold a real root.
                        let native_required = expected(activation.clone(), locator);
                        let native_verified = boot::restore_forward::verify_native_separate_repomap_v1(
                            &native_required, &config, search, native,
                            authority.io.as_ref(), locked, budget,
                        )?.ok_or(IndexError::SnapshotUnavailable)?;
                        let native_held = native_verified.separate.as_ref()
                            .ok_or(IndexError::SnapshotUnavailable)?;
                        native_held.require_repomap_locator_v1(locator, budget)?;
                        native_verified.files.require_current_v1(
                            native_held.root(), native_held.objects(), budget,
                        )?;
                        drop(native_verified);
                        let native_na = NativeCutoverEvidenceLegacyRepoMapV1::NotApplicable {
                            reason_code: "not-applicable".into(),
                            policy_digest: NativeCutoverDigest64V1([b'a'; 64]),
                            product_scope_digest: NativeCutoverDigest64V1([b'b'; 64]),
                        };
                        anyhow::ensure!(matches!(
                            boot::restore_forward::verify_native_separate_repomap_v1(
                                &native_na, &config, search, native,
                                authority.io.as_ref(), locked, budget,
                            ),
                            Err(error) if error.downcast_ref::<IndexError>() == Some(&IndexError::DigestConflict)
                        ), "Native NotApplicable accepted an active saved RepoMap selection");
                        let native_foreign_locator = expected(
                            activation.clone(), NativeCutoverDigest64V1([b'a'; 64]),
                        );
                        anyhow::ensure!(matches!(
                            boot::restore_forward::verify_native_separate_repomap_v1(
                                &native_foreign_locator, &config, search, native,
                                authority.io.as_ref(), locked, budget,
                            ),
                            Err(error) if error.downcast_ref::<IndexError>() == Some(&IndexError::DigestConflict)
                        ), "Native Required accepted a foreign physical locator");
                        let mut foreign = activation.clone();
                        foreign.mutation.activation_epoch += 1;
                        let wrong_receipt = verify(&expected(foreign, locator), budget);
                        anyhow::ensure!(
                            matches!(wrong_receipt, Err(error) if matches!(
                                error.downcast_ref::<ExistingFileReadFailureV1>(),
                                Some(ExistingFileReadFailureV1::AdmissionOrIo(IndexError::DigestConflict))
                            )),
                            "foreign activation epoch passed or refused for another reason"
                        );
                        let wrong_locator =
                            expected(activation.clone(), NativeCutoverDigest64V1([b'a'; 64]));
                        anyhow::ensure!(
                            matches!(verify(&wrong_locator, budget), Err(error)
                    if error.downcast_ref::<IndexError>() == Some(&IndexError::DigestConflict)),
                            "foreign locator passed or refused for another reason"
                        );
                        let valid = verify(&expected(activation, locator), budget)?
                            .ok_or(IndexError::SnapshotUnavailable)?;
                        anyhow::ensure!(
                            quanta_index_searchd::app::runtime::StateRootLease::acquire_existing(
                                &repomap
                            )
                            .is_err(),
                            "verified RepoMap released its root lease early",
                        );
                        let held = valid
                            .separate
                            .as_ref()
                            .ok_or(IndexError::SnapshotUnavailable)?;
                        held.require_repomap_locator_v1(locator, budget)?;
                        held.require_current_and_retired(budget)?;
                        legacy.retire_catalog(budget)?;
                        let marker_path = repomap.join(PRODUCT_ROOT_IDENTITY_FILE_V2);
                        let changed =
                            encode_product_root_identity_v2(ProductRootKindV2::RepoMap, [12; 32]);
                        fs::write(&marker_path, changed)?;
                        let result = boot::publish_with_held_recovery_roots_v1(
                            &authority,
                            Some(&legacy),
                            &mut |budget| legacy.require_current_and_retired(budget),
                            Some(&valid),
                            &config,
                            locked,
                            budget,
                            &mut |budget| Ok(budget.checkpoint()?),
                        );
                        fs::write(&marker_path, marker)?;
                        Ok(result)
                    };
                boot::with_recovery_boot_authority(&config, budget, verify_and_refuse)
                    .map_err(DeploymentSelectionErrorV1::RefusedBeforeRename)?
            },
        ))
    })?;
    match result {
        Err(DeploymentSelectionErrorV1::RefusedBeforeRename(error))
            if error.downcast_ref::<IndexError>() == Some(&IndexError::DigestConflict) => {}
        Err(
            DeploymentSelectionErrorV1::RefusedBeforeRename(error)
            | DeploymentSelectionErrorV1::PersistenceUncertain(error),
        ) => return Err(error.into()),
        Ok(_) => return Err("physical-only test unexpectedly published".into()),
    }
    require(
        fs::read(selection)? == before,
        "physical-only test changed selection",
    )?;
    drop(quanta_index_searchd::app::runtime::StateRootLease::acquire_existing(&repomap)?);
    Ok(())
}
