use super::*;

#[test]
fn repeated_work_status_reads_observe_claim_without_catalog_writes() -> TestResult {
    let root = tempfile::tempdir()?;
    let (catalog, genesis) = setup(root.path())?;
    let intent = owner_work(231);
    let _queued = publish_work_next(&catalog, &genesis, 1, &intent, 12_231, 231)?;

    check_eq!(
        catalog.work_status(&intent, &admission_budget())?,
        WorkStatus::Pending
    );
    let changes_before = catalog.lock()?.total_changes();
    for _ in 0..3 {
        check_eq!(
            catalog.work_status(&intent, &admission_budget())?,
            WorkStatus::Pending
        );
    }
    let changes_after_reads = catalog.lock()?.total_changes();
    check_eq!(changes_after_reads, changes_before);

    let claim = catalog.claim_work(&intent, Duration::from_secs(30), &admission_budget())?;
    let changes_after_claim = catalog.lock()?.total_changes();
    for _ in 0..3 {
        check_eq!(
            catalog.work_status(&intent, &admission_budget())?,
            WorkStatus::Claimed(claim)
        );
    }
    let changes_after_claim_reads = catalog.lock()?.total_changes();
    check_eq!(changes_after_claim_reads, changes_after_claim);
    Ok(())
}

#[test]
fn warmed_work_reads_reject_each_missing_or_corrupt_root_custody() -> TestResult {
    for target in ["source", "projection", "prepared"] {
        let root = tempfile::tempdir()?;
        let (catalog, genesis) = setup(root.path())?;
        let source = object(232, vec![]);
        let projection = object(233, vec![]);
        let artifact = object(234, vec![]);
        let mut intent = owner_work(235);
        let WorkScope::Owner { expected, .. } = &mut intent.scope else {
            return Err("owner fixture changed".into());
        };
        expected.source = Some(source.reference);
        expected.projection = Some(projection.reference);

        let attempt = admit_new(&catalog, operation(12_232, 232))?;
        reserve_confirm(&catalog, attempt, &[source.clone(), projection.clone()])?;
        let queued = view(1, genesis.profile, None);
        check_eq!(
            commit_actions(
                &catalog,
                PreparedPublication {
                    attempt,
                    expected: genesis.identity,
                    next: queued.clone(),
                    work_actions: WorkActionSet::empty(),
                    complete_work: None,
                    source_events: vec![],
                    projection_events: vec![],
                    expected_source: None,
                    source_mode: SourceOperationMode::Unchanged,
                },
                &[WorkAction::Create(intent)],
            )?,
            OperationStatus::Published(queued.identity)
        );
        check_eq!(
            catalog.work_status(&intent, &admission_budget())?,
            WorkStatus::Pending
        );
        let claim = catalog.claim_work(&intent, Duration::from_secs(30), &admission_budget())?;
        check_eq!(
            catalog.work_status(&intent, &admission_budget())?,
            WorkStatus::Claimed(claim)
        );

        let artifact_attempt = admit_new(&catalog, operation(12_233, 233))?;
        reserve_confirm(&catalog, artifact_attempt, std::slice::from_ref(&artifact))?;
        catalog.stage_work(&claim, owner_result(&intent, artifact.reference)?)?;
        check_eq!(
            catalog.work_status(&intent, &admission_budget())?,
            WorkStatus::Claimed(claim)
        );

        let reference = match target {
            "source" => source.reference,
            "projection" => projection.reference,
            "prepared" => artifact.reference,
            _ => return Err("unknown custody target".into()),
        };
        let key = work_key(&intent)?;
        let object = object_key(reference);
        let changed = {
            let guard = catalog.lock()?;
            if target == "prepared" {
                guard.execute(
                    "UPDATE native_work_object_v1 SET row_digest=zeroblob(32) WHERE work=?1 AND object=?2",
                    params![&key, &object],
                )?
            } else {
                guard.execute(
                    "DELETE FROM native_work_object_v1 WHERE work=?1 AND object=?2",
                    params![&key, &object],
                )?
            }
        };
        check_eq!(changed, 1);
        let expected_reason = if target == "prepared" {
            "work root custody digest mismatch"
        } else {
            "work root custody missing"
        };
        check!(matches!(
            catalog.work_status(&intent, &admission_budget()),
            Err(IndexError::Storage(reason)) if reason == expected_reason
        ));
    }
    Ok(())
}
