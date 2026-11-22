use super::*;

fn selected_catalog(root: &Path) -> Result<(SqliteCatalog, NativeBootManifest), Box<dyn Error>> {
    let (catalog, genesis) = setup(root)?;
    let original = operation(31_001, 191);
    let attempt = admit_new(&catalog, original)?;
    let published = view(1, genesis.profile, None);
    check_eq!(
        publish(&catalog, attempt, &genesis, published.clone())?,
        OperationStatus::Published(published.identity)
    );
    let baseline = catalog.select_native_boot_manifest(original.key, &mut admission_budget())?;
    Ok((catalog, baseline))
}

#[test]
fn offline_rollback_accepts_unchanged_committed_receipt_after_ack_loss_and_reopen() -> TestResult {
    let root = tempfile::tempdir()?;
    let (catalog, baseline) = selected_catalog(root.path())?;
    let key = baseline.operation.key;
    // Lose the caller's ACK and reopen the actual durable Catalog. Original
    // operation identity, rather than a new publication, reconciles the retry.
    drop(catalog);
    let catalog = SqliteCatalog::open(root.path(), Duration::ZERO)?;
    let guard = catalog.lock()?;
    check_eq!(
        read_operation(&guard, key)?
            .ok_or("missing original receipt")?
            .status,
        OperationStatus::Published(baseline.publication.identity)
    );
    read_only::check_rollback(&guard, &baseline, &mut admission_budget())?;
    read_only::check_rollback(&guard, &baseline, &mut admission_budget())?;
    check_eq!(
        current_view(&guard, key.workspace)?,
        Some(baseline.publication.clone())
    );
    Ok(())
}

#[test]
fn offline_rollback_refuses_open_ingress_and_pending_operation() -> TestResult {
    for admitted in [false, true] {
        let root = tempfile::tempdir()?;
        let (catalog, baseline) = selected_catalog(root.path())?;
        let later = operation(31_002, 192);
        if admitted {
            let _attempt = admit_new(&catalog, later)?;
        } else {
            let _ingress = catalog.open_ingress(later.key)?;
        }
        let guard = catalog.lock()?;
        check!(matches!(
            read_only::check_rollback(&guard, &baseline, &mut admission_budget()),
            Err(IndexError::Conflict)
        ));
        check_eq!(
            current_view(&guard, later.key.workspace)?,
            Some(baseline.publication.clone())
        );
    }
    Ok(())
}

#[test]
fn offline_rollback_refuses_publication_after_baseline_without_mutation() -> TestResult {
    let root = tempfile::tempdir()?;
    let (catalog, baseline) = selected_catalog(root.path())?;
    let later = publish_plain_next(&catalog, &baseline.publication, 2, 193)?;
    let guard = catalog.lock()?;
    check!(matches!(
        read_only::check_rollback(&guard, &baseline, &mut admission_budget()),
        Err(IndexError::Conflict)
    ));
    check_eq!(current_view(&guard, later.identity.workspace)?, Some(later));
    check_eq!(boot_record::read_stored(&guard)?, Some(baseline));
    Ok(())
}

#[test]
fn offline_rollback_refuses_pending_work_and_corrupt_terminal_shadow() -> TestResult {
    let root = tempfile::tempdir()?;
    let (catalog, first) = selected_catalog(root.path())?;
    let work = owner_work(194);
    let published = publish_work_next(&catalog, &first.publication, 2, &work, 31_004, 195)?;
    let baseline =
        catalog.select_native_boot_manifest(operation(31_004, 195).key, &mut admission_budget())?;
    check_eq!(baseline.publication, published);
    let guard = catalog.lock()?;
    check!(matches!(
        read_only::check_rollback(&guard, &baseline, &mut admission_budget()),
        Err(IndexError::Conflict)
    ));
    // The canonical payload still says Pending. A forged terminal shadow
    // must fail row authentication rather than authorize a backward switch.
    check_eq!(guard.execute("UPDATE native_work_v1 SET status=2", [])?, 1);
    check!(read_only::check_rollback(&guard, &baseline, &mut admission_budget()).is_err());
    check_eq!(
        current_view(&guard, published.identity.workspace)?,
        Some(published)
    );
    check_eq!(boot_record::read_stored(&guard)?, Some(baseline));
    Ok(())
}

#[test]
fn offline_rollback_keeps_original_baseline_after_boot_reissue() -> TestResult {
    let root = tempfile::tempdir()?;
    let (catalog, original) = selected_catalog(root.path())?;
    let later = publish_plain_next(&catalog, &original.publication, 2, 196)?;
    let reissued =
        catalog.select_native_boot_manifest(operation(922, 196).key, &mut admission_budget())?;
    check_eq!(reissued.publication, later);
    let guard = catalog.lock()?;
    check!(matches!(
        read_only::check_rollback(&guard, &original, &mut admission_budget()),
        Err(IndexError::Conflict)
    ));
    check_eq!(boot_record::read_stored(&guard)?, Some(reissued));
    check_eq!(current_view(&guard, later.identity.workspace)?, Some(later));
    Ok(())
}

#[test]
fn offline_rollback_preserves_original_cancellation_without_mutation() -> TestResult {
    let root = tempfile::tempdir()?;
    let (catalog, baseline) = selected_catalog(root.path())?;
    let guard = catalog.lock()?;
    let mut budget = admission_budget();
    budget.request().cancel_handle().cancel();
    check!(matches!(
        read_only::check_rollback(&guard, &baseline, &mut budget),
        Err(IndexError::Cancelled)
    ));
    check_eq!(boot_record::read_stored(&guard)?, Some(baseline.clone()));
    check_eq!(
        current_view(&guard, baseline.publication.identity.workspace)?,
        Some(baseline.publication)
    );
    Ok(())
}

#[test]
fn offline_published_full_relation_survives_retired_view_and_request_object() -> TestResult {
    let root = tempfile::tempdir()?;
    let (catalog, full) = selected_catalog(root.path())?;
    let next = publish_plain_next(&catalog, &full.publication, 2, 197)?;
    let installed =
        catalog.select_native_boot_manifest(operation(922, 197).key, &mut admission_budget())?;
    check_eq!(installed.publication, next);
    check_eq!(
        catalog.advance_exact_history_floor(workspace(), 7, 2, &mut admission_budget())?,
        2
    );
    let retired =
        catalog.retire_exact_history_page(workspace(), 7, None, 8, &mut admission_budget())?;
    check!(retired.retired > 0);
    let released =
        catalog.release_terminal_custody(full.operation.key, 256, &mut admission_budget())?;
    check!(!released.has_more);
    // Model completed reclamation of the now unprotected request registration,
    // using the same lower fixture boundary as terminal-operation replay tests.
    let guard = catalog.lock()?;
    check_eq!(
        guard.execute(
            "DELETE FROM native_object_v1 WHERE object=?1",
            params![object_key(full.operation.request)]
        )?,
        1
    );
    check!(matches!(
        boot_record::load_publication(&guard, full.publication.identity, &mut admission_budget()),
        Err(IndexError::SnapshotUnavailable)
    ));
    check_eq!(
        read_only::read_published_operation(&guard, full.operation.key, &mut admission_budget())?,
        (full.operation, full.publication.clone())
    );
    // The installed selection, rather than the retired FULL view, is the
    // semantic rollback baseline after these non-publication mutations.
    read_only::check_rollback(&guard, &installed, &mut admission_budget())?;
    check_eq!(current_view(&guard, next.identity.workspace)?, Some(next));
    Ok(())
}

#[test]
fn offline_published_relation_refuses_pending_unknown_and_original_cancellation() -> TestResult {
    let root = tempfile::tempdir()?;
    let (catalog, full) = selected_catalog(root.path())?;
    let pending = operation(31_009, 199);
    let _attempt = admit_new(&catalog, pending)?;
    let guard = catalog.lock()?;
    check!(matches!(
        read_only::read_published_operation(&guard, pending.key, &mut admission_budget()),
        Err(IndexError::Conflict)
    ));
    check!(matches!(
        read_only::read_published_operation(
            &guard,
            operation(31_010, 200).key,
            &mut admission_budget()
        ),
        Err(IndexError::SnapshotUnavailable)
    ));
    let mut budget = admission_budget();
    budget.request().cancel_handle().cancel();
    check!(matches!(
        read_only::read_published_operation(&guard, full.operation.key, &mut budget),
        Err(IndexError::Cancelled)
    ));
    check_eq!(boot_record::read_stored(&guard)?, Some(full));
    Ok(())
}
