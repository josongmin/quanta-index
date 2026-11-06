use super::*;
use quanta_index_core::index::ports::{
    CatalogReclaimPort, InputStagingPort, ManagedObjectInstallPort,
};
use quanta_index_core::index::{ObjectInstallScope, ReclaimCandidate, ReclaimKind};

fn queue_count(catalog: &SqliteCatalog, object: ObjectRef) -> Result<i64, Box<dyn Error>> {
    let guard = catalog.lock()?;
    Ok(guard.query_row(
        "SELECT COUNT(*) FROM native_gc_queue_v1 WHERE object=?1",
        params![object_key(object)],
        |row| row.get(0),
    )?)
}

fn object_count(
    catalog: &SqliteCatalog,
    table: &'static str,
    object: ObjectRef,
) -> Result<i64, Box<dyn Error>> {
    let guard = catalog.lock()?;
    let query = format!("SELECT COUNT(*) FROM {table} WHERE object=?1");
    Ok(guard.query_row(&query, params![object_key(object)], |row| row.get(0))?)
}

fn candidate(
    catalog: &SqliteCatalog,
    object: ObjectRef,
    kind: ReclaimKind,
) -> Result<ReclaimCandidate, Box<dyn Error>> {
    let guard = catalog.lock()?;
    let generation: i64 = guard.query_row(
        "SELECT generation FROM native_gc_object_v1 WHERE object=?1",
        params![object_key(object)],
        |row| row.get(0),
    )?;
    drop(guard);
    Ok(ReclaimCandidate {
        object,
        generation: u64::try_from(generation)?,
        kind,
    })
}

fn managed_attempt(
    catalog: &SqliteCatalog,
    operation: ImmutableOperation,
) -> Result<AttemptRef, Box<dyn Error>> {
    let request = RegisteredObject::unscoped(operation.request, 1, vec![]);
    let ingress = catalog.open_ingress(operation.key)?;
    catalog.reserve_managed(
        ObjectInstallScope::Ingress(ingress),
        std::slice::from_ref(&request),
        &mut admission_budget(),
    )?;
    catalog.confirm_managed(
        ObjectInstallScope::Ingress(ingress),
        std::slice::from_ref(&request),
        &mut admission_budget(),
    )?;
    catalog.seal_inputs(ingress, operation.request)?;
    match catalog.admit(operation, ingress, &[], &mut admission_budget())? {
        AdmissionOutcome::New(attempt) => Ok(attempt),
        AdmissionOutcome::Existing(status) => {
            Err(format!("expected fresh managed attempt, got {status:?}").into())
        }
    }
}

fn release_terminal(catalog: &SqliteCatalog, key: OperationKey) -> TestResult {
    // The fixture has at most a request and one result. A bounded page must
    // release both, with no hidden cleanup after the terminal row disappears.
    let page = catalog.release_terminal_custody(key, 8, &mut admission_budget())?;
    check!(!page.has_more);
    Ok(())
}

fn seed_queued_lifecycle(catalog: &SqliteCatalog, object: ObjectRef) -> TestResult {
    let mut guard = catalog.lock()?;
    let tx = begin(&mut guard)?;
    reclaim::reserve_object(&tx, object)?;
    check!(reclaim::enqueue(&tx, object)?);
    tx.commit()?;
    drop(guard);
    Ok(())
}

fn ensure_queued_candidate(catalog: &SqliteCatalog, object: ObjectRef) -> TestResult {
    let mut guard = catalog.lock()?;
    let tx = begin(&mut guard)?;
    let _inserted = reclaim::enqueue(&tx, object)?;
    tx.commit()?;
    drop(guard);
    check_eq!(queue_count(catalog, object)?, 1);
    Ok(())
}

#[test]
fn fresh_attempt_reserve_and_confirm_stay_out_of_queue_until_terminal_release() -> TestResult {
    let (root, mut catalog, _ingress) = managed_adoption_fixture()?;
    for (ordinal, confirmed) in [(0_u64, false), (1, true)] {
        let operation = operation(22000 + ordinal, 170 + u8::try_from(ordinal)?);
        let attempt = managed_attempt(&catalog, operation)?;
        let result = object(230 + u8::try_from(ordinal)?, vec![]);
        catalog.reserve_managed(
            ObjectInstallScope::Attempt(attempt),
            std::slice::from_ref(&result),
            &mut admission_budget(),
        )?;
        check_eq!(queue_count(&catalog, result.reference)?, 0);
        check_eq!(
            object_count(&catalog, "native_object_intent_v1", result.reference)?,
            1
        );
        if confirmed {
            catalog.confirm_managed(
                ObjectInstallScope::Attempt(attempt),
                std::slice::from_ref(&result),
                &mut admission_budget(),
            )?;
            check_eq!(queue_count(&catalog, result.reference)?, 0);
            check_eq!(
                object_count(&catalog, "native_attempt_object_v1", result.reference)?,
                1
            );
        }
        // The intent or confirmed protection is durable independently of the
        // process that performed the reservation and physical write.
        drop(catalog);
        catalog = open(root.path())?;
        check_eq!(queue_count(&catalog, result.reference)?, 0);
        check_eq!(
            object_count(&catalog, "native_object_intent_v1", result.reference)?,
            1
        );
        if confirmed {
            check_eq!(
                object_count(&catalog, "native_attempt_object_v1", result.reference)?,
                1
            );
        }
        check_eq!(
            catalog.refuse(attempt, Refusal::Conflict)?,
            OperationStatus::Refused(Refusal::Conflict)
        );
        release_terminal(&catalog, operation.key)?;
        check_eq!(queue_count(&catalog, result.reference)?, 1);
    }
    drop(catalog);
    let catalog = open(root.path())?;
    for (ordinal, kind) in [ReclaimKind::Provisional, ReclaimKind::Registered]
        .into_iter()
        .enumerate()
    {
        let result = object(230 + u8::try_from(ordinal)?, vec![]);
        check_eq!(queue_count(&catalog, result.reference)?, 1);
        check!(
            catalog
                .retire_candidate(
                    candidate(&catalog, result.reference, kind)?,
                    &mut admission_budget()
                )?
                .is_some()
        );
    }
    Ok(())
}

#[test]
fn fresh_ingress_reserve_and_confirm_enqueue_only_on_discard() -> TestResult {
    let (root, catalog, _ingress) = managed_adoption_fixture()?;
    for (ordinal, confirmed) in [(0_u64, false), (1, true)] {
        let ingress =
            catalog.open_ingress(operation(22100 + ordinal, 172 + u8::try_from(ordinal)?).key)?;
        let result = object(232 + u8::try_from(ordinal)?, vec![]);
        catalog.reserve_managed(
            ObjectInstallScope::Ingress(ingress),
            std::slice::from_ref(&result),
            &mut admission_budget(),
        )?;
        check_eq!(queue_count(&catalog, result.reference)?, 0);
        if confirmed {
            catalog.confirm_managed(
                ObjectInstallScope::Ingress(ingress),
                std::slice::from_ref(&result),
                &mut admission_budget(),
            )?;
            check_eq!(queue_count(&catalog, result.reference)?, 0);
            check_eq!(
                object_count(&catalog, "native_ingress_object_v1", result.reference)?,
                1
            );
        }
        catalog.discard_inputs(ingress, &mut admission_budget())?;
        check_eq!(queue_count(&catalog, result.reference)?, 1);
    }
    drop(catalog);
    let catalog = open(root.path())?;
    for ordinal in 0..2_u8 {
        check_eq!(
            queue_count(&catalog, object(232 + ordinal, vec![]).reference)?,
            1
        );
    }
    Ok(())
}

fn existing_queue_is_authenticated_and_late_corruption_rolls_back(
    catalog: &SqliteCatalog,
    scope: ObjectInstallScope,
    good: &RegisteredObject,
    bad: &RegisteredObject,
) -> TestResult {
    let batch = [(*good).clone(), (*bad).clone()];
    seed_queued_lifecycle(catalog, good.reference)?;
    seed_queued_lifecycle(catalog, bad.reference)?;
    {
        let guard = catalog.lock()?;
        check_eq!(
            guard.execute(
                "UPDATE native_gc_queue_v1 SET row_digest=zeroblob(32) WHERE object=?1",
                params![object_key(bad.reference)],
            )?,
            1
        );
    }
    check!(matches!(
        catalog.reserve_managed(scope, &batch, &mut admission_budget()),
        Err(IndexError::Storage(_))
    ));
    // The good candidate's dequeue, provisional row and intent must all roll
    // back when the later candidate fails queue authentication.
    check_eq!(queue_count(catalog, good.reference)?, 1);
    check_eq!(queue_count(catalog, bad.reference)?, 1);
    for object in [good.reference, bad.reference] {
        check_eq!(
            object_count(catalog, "native_object_install_v1", object)?,
            0
        );
        check_eq!(object_count(catalog, "native_object_intent_v1", object)?, 0);
        check_eq!(
            object_count(catalog, "native_ingress_intent_v1", object)?,
            0
        );
    }
    {
        let guard = catalog.lock()?;
        let key = object_key(bad.reference);
        check_eq!(
            guard.execute(
                "UPDATE native_gc_queue_v1 SET row_digest=?1 WHERE object=?2",
                params![fields_digest(b"qidx.native.gc.queue.v1", &key)?, key],
            )?,
            1
        );
    }
    catalog.reserve_managed(scope, &batch, &mut admission_budget())?;
    check_eq!(queue_count(catalog, good.reference)?, 0);
    check_eq!(queue_count(catalog, bad.reference)?, 0);
    catalog.confirm_managed(scope, &batch, &mut admission_budget())?;
    check_eq!(queue_count(catalog, good.reference)?, 0);
    check_eq!(queue_count(catalog, bad.reference)?, 0);
    Ok(())
}

#[test]
fn preexisting_queue_is_consumed_atomically_by_ingress_and_attempt() -> TestResult {
    let (_root, catalog, _ingress) = managed_adoption_fixture()?;
    let ingress = catalog.open_ingress(operation(22200, 174).key)?;
    existing_queue_is_authenticated_and_late_corruption_rolls_back(
        &catalog,
        ObjectInstallScope::Ingress(ingress),
        &object(234, vec![]),
        &object(235, vec![]),
    )?;
    let attempt = managed_attempt(&catalog, operation(22201, 175))?;
    existing_queue_is_authenticated_and_late_corruption_rolls_back(
        &catalog,
        ObjectInstallScope::Attempt(attempt),
        &object(236, vec![]),
        &object(237, vec![]),
    )
}

#[test]
fn ingress_admission_transfers_custody_before_old_ingress_is_removed() -> TestResult {
    let (_root, catalog, _unused) = managed_adoption_fixture()?;
    let operation = operation(22300, 176);
    let request = RegisteredObject::unscoped(operation.request, 1, vec![]);
    let ingress = catalog.open_ingress(operation.key)?;
    catalog.reserve_managed(
        ObjectInstallScope::Ingress(ingress),
        std::slice::from_ref(&request),
        &mut admission_budget(),
    )?;
    catalog.confirm_managed(
        ObjectInstallScope::Ingress(ingress),
        std::slice::from_ref(&request),
        &mut admission_budget(),
    )?;
    let leftover = object(240, vec![]);
    catalog.reserve_managed(
        ObjectInstallScope::Ingress(ingress),
        std::slice::from_ref(&leftover),
        &mut admission_budget(),
    )?;
    check_eq!(queue_count(&catalog, leftover.reference)?, 0);
    catalog.seal_inputs(ingress, operation.request)?;
    check_eq!(
        object_count(&catalog, "native_ingress_object_v1", request.reference)?,
        1
    );
    check_eq!(
        object_count(&catalog, "native_operation_object_v1", request.reference)?,
        0
    );
    let attempt = match catalog.admit(operation, ingress, &[], &mut admission_budget())? {
        AdmissionOutcome::New(attempt) => attempt,
        AdmissionOutcome::Existing(status) => {
            return Err(format!("expected fresh managed admission, got {status:?}").into());
        }
    };
    check_eq!(
        object_count(&catalog, "native_ingress_object_v1", request.reference)?,
        0
    );
    check_eq!(
        object_count(&catalog, "native_operation_object_v1", request.reference)?,
        1
    );
    check_eq!(queue_count(&catalog, request.reference)?, 0);
    check_eq!(queue_count(&catalog, leftover.reference)?, 1);
    check_eq!(
        object_count(&catalog, "native_operation_object_v1", leftover.reference)?,
        0
    );
    // Admission may reenqueue while other protection exists. Retirement must
    // still refuse an explicitly queued candidate until final release.
    ensure_queued_candidate(&catalog, request.reference)?;
    check!(
        catalog
            .retire_candidate(
                candidate(&catalog, request.reference, ReclaimKind::Registered)?,
                &mut admission_budget(),
            )?
            .is_none()
    );
    check_eq!(
        catalog.refuse(attempt, Refusal::Conflict)?,
        OperationStatus::Refused(Refusal::Conflict)
    );
    release_terminal(&catalog, operation.key)?;
    check_eq!(queue_count(&catalog, request.reference)?, 1);
    Ok(())
}

#[test]
fn corrupt_transferred_operation_custody_rolls_back_admission_and_old_protection() -> TestResult {
    let (_root, catalog, _unused) = managed_adoption_fixture()?;
    let operation = operation(22301, 178);
    let request = RegisteredObject::unscoped(operation.request, 1, vec![]);
    let ingress = catalog.open_ingress(operation.key)?;
    catalog.reserve_managed(
        ObjectInstallScope::Ingress(ingress),
        std::slice::from_ref(&request),
        &mut admission_budget(),
    )?;
    catalog.confirm_managed(
        ObjectInstallScope::Ingress(ingress),
        std::slice::from_ref(&request),
        &mut admission_budget(),
    )?;
    catalog.seal_inputs(ingress, operation.request)?;
    check_eq!(queue_count(&catalog, request.reference)?, 0);
    {
        let guard = catalog.lock()?;
        guard.execute_batch(
            "CREATE TEMP TRIGGER corrupt_transferred_operation_custody
             AFTER INSERT ON native_operation_object_v1
             BEGIN
               UPDATE native_operation_object_v1 SET row_digest=zeroblob(32)
               WHERE operation_id=NEW.operation_id AND object=NEW.object;
             END",
        )?;
    }
    check!(matches!(
        catalog.admit(operation, ingress, &[], &mut admission_budget()),
        Err(IndexError::Storage(_))
    ));
    // The trigger runs inside the admission transaction. The failed digest
    // proof must undo the operation row, custody transfer and ingress cleanup.
    check_eq!(
        object_count(&catalog, "native_operation_object_v1", request.reference)?,
        0
    );
    check_eq!(
        object_count(&catalog, "native_ingress_object_v1", request.reference)?,
        1
    );
    check_eq!(
        object_count(&catalog, "native_ingress_intent_v1", request.reference)?,
        1
    );
    check_eq!(
        object_count(&catalog, "native_object_v1", request.reference)?,
        1
    );
    check_eq!(
        object_count(&catalog, "native_object_install_v1", request.reference)?,
        0
    );
    check_eq!(queue_count(&catalog, request.reference)?, 0);
    {
        let guard = catalog.lock()?;
        check_eq!(
            guard.query_row(
                "SELECT COUNT(*) FROM native_operation_v1 WHERE operation=?1",
                params![operation_key(operation.key)?],
                |row| row.get::<_, i64>(0),
            )?,
            0
        );
        guard.execute_batch("DROP TRIGGER corrupt_transferred_operation_custody")?;
    }
    let attempt = match catalog.admit(operation, ingress, &[], &mut admission_budget())? {
        AdmissionOutcome::New(attempt) => attempt,
        AdmissionOutcome::Existing(status) => {
            return Err(format!("admission rollback lost replay: {status:?}").into());
        }
    };
    check_eq!(queue_count(&catalog, request.reference)?, 0);
    check_eq!(
        object_count(&catalog, "native_operation_object_v1", request.reference)?,
        1
    );
    check_eq!(
        inspect_with_test_budget(&catalog, operation.key)?,
        OperationStatus::Pending(attempt)
    );
    Ok(())
}

#[test]
fn shared_ingress_owner_blocks_retirement_until_last_release() -> TestResult {
    let (_root, catalog, first) = managed_adoption_fixture()?;
    let shared = object(238, vec![]);
    catalog.reserve_managed(
        ObjectInstallScope::Ingress(first),
        std::slice::from_ref(&shared),
        &mut admission_budget(),
    )?;
    catalog.confirm_managed(
        ObjectInstallScope::Ingress(first),
        std::slice::from_ref(&shared),
        &mut admission_budget(),
    )?;
    let operation = operation(22400, 238);
    let second = catalog.open_ingress(operation.key)?;
    catalog.adopt_registered_inputs(
        second,
        std::slice::from_ref(&shared),
        &mut admission_budget(),
    )?;
    {
        let guard = catalog.lock()?;
        let target_intents: i64 = guard.query_row(
            "SELECT COUNT(*) FROM native_ingress_intent_v1 WHERE operation=?1 AND object=?2",
            params![operation_key(operation.key)?, object_key(shared.reference)],
            |row| row.get(0),
        )?;
        drop(guard);
        check_eq!(target_intents, 0);
    }
    catalog.seal_inputs(second, shared.reference)?;
    let attempt = match catalog.admit(operation, second, &[], &mut admission_budget())? {
        AdmissionOutcome::New(attempt) => attempt,
        AdmissionOutcome::Existing(status) => {
            return Err(format!("adopted request admission was not new: {status:?}").into());
        }
    };
    check_eq!(queue_count(&catalog, shared.reference)?, 0);
    catalog.discard_inputs(first, &mut admission_budget())?;
    ensure_queued_candidate(&catalog, shared.reference)?;
    let candidate = candidate(&catalog, shared.reference, ReclaimKind::Registered)?;
    check!(
        catalog
            .retire_candidate(candidate, &mut admission_budget())?
            .is_none()
    );
    check_eq!(
        object_count(&catalog, "native_operation_object_v1", shared.reference)?,
        1
    );
    check_eq!(
        catalog.refuse(attempt, Refusal::Conflict)?,
        OperationStatus::Refused(Refusal::Conflict)
    );
    release_terminal(&catalog, operation.key)?;
    check_eq!(queue_count(&catalog, shared.reference)?, 1);
    check!(
        catalog
            .retire_candidate(candidate, &mut admission_budget())?
            .is_some()
    );
    Ok(())
}
