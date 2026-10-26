// Catalog phase attribution preserves the real managed transaction outcome.
mod managed_install_trace_tests {
    use super::*;
    use quanta_index_core::index::ports::ManagedObjectInstallPort;
    use quanta_index_core::{
        DiagnosticCaptureIdV1, DiagnosticClockV1, DiagnosticContextV1, DiagnosticEmitV1,
        DiagnosticEventKindV1, DiagnosticEventSinkV1, DiagnosticLaneV1, DiagnosticRootKindV1,
        DiagnosticStageV1, DiagnosticTerminalV1, RequestBudgetV1,
    };
    use std::sync::Mutex;

    #[derive(Debug, Default)]
    struct StepClock(AtomicU64);
    impl DiagnosticClockV1 for StepClock {
        fn now_ns(&self) -> u64 {
            self.0.fetch_add(1, Ordering::Relaxed)
        }
    }

    #[derive(Debug)]
    struct Events {
        rows: Mutex<Vec<DiagnosticEmitV1>>,
        accept: bool,
    }
    impl DiagnosticEventSinkV1 for Events {
        fn try_emit(&self, event: DiagnosticEmitV1) -> bool {
            let Ok(mut rows) = self.rows.try_lock() else {
                return false;
            };
            if !self.accept || rows.len() == 128 {
                return false;
            }
            rows.push(event);
            true
        }
    }

    struct Run {
        result: Result<(), IndexError>,
        usage: WorkUsage,
        clocks: u64,
        events: Vec<DiagnosticEmitV1>,
        // Permanent, provisional and attempt-protected fixture object counts.
        rows: [i64; 3],
    }

    fn counts(
        catalog: &SqliteCatalog,
        objects: &[RegisteredObject; 2],
    ) -> Result<[i64; 3], Box<dyn Error>> {
        let guard = catalog.lock().map_err(|error| error.to_string())?;
        let mut counts = [0; 3];
        for (count, table) in counts.iter_mut().zip([
            "native_object_v1",
            "native_object_install_v1",
            "native_attempt_object_v1",
        ]) {
            *count = guard.query_row(
                &format!("SELECT COUNT(*) FROM {table} WHERE object=?1 OR object=?2"),
                params![
                    object_key(objects[0].reference),
                    object_key(objects[1].reference)
                ],
                |row| row.get(0),
            )?;
        }
        drop(guard);
        Ok(counts)
    }

    struct Observation {
        budget: WorkBudget,
        clock: Arc<StepClock>,
        events: Arc<Events>,
        root: Option<quanta_index_core::DiagnosticSpanGuardV1>,
    }

    fn request(record: bool, accept: bool) -> Result<Observation, Box<dyn Error>> {
        let clock = Arc::new(StepClock::default());
        let events = Arc::new(Events {
            rows: Mutex::new(Vec::new()),
            accept,
        });
        let context = DiagnosticContextV1::new(
            DiagnosticCaptureIdV1([93; 16]),
            clock.clone(),
            events.clone(),
        );
        let root = if record {
            Some(context.enter_root(DiagnosticRootKindV1::Operation, DiagnosticLaneV1(1))?)
        } else {
            None
        };
        let request = RequestBudgetV1::for_duration(Duration::from_secs(30));
        let request = match root.as_ref() {
            Some(root) => request.with_diagnostic_context(context.child_context(root)),
            None => request,
        };
        let limits = WorkLimits {
            reads: 100_000,
            writes: 100_000,
            bytes: 32 * 1024 * 1024,
            mutations: 100_000,
        };
        Ok(Observation {
            budget: WorkBudget::with_request(limits, request),
            clock,
            events,
            root,
        })
    }

    fn managed_attempt(catalog: &SqliteCatalog) -> Result<AttemptRef, Box<dyn Error>> {
        let operation = operation(14001, 152);
        let request = RegisteredObject::unscoped(operation.request, 1, vec![]);
        let ingress = catalog.open_ingress(operation.key)?;
        let mut budget = admission_budget();
        catalog.reserve_managed(
            ObjectInstallScope::Ingress(ingress),
            std::slice::from_ref(&request),
            &mut budget,
        )?;
        catalog.confirm_managed(
            ObjectInstallScope::Ingress(ingress),
            std::slice::from_ref(&request),
            &mut budget,
        )?;
        catalog.seal_inputs(ingress, operation.request)?;
        match catalog.admit(operation, ingress, &[], &mut budget)? {
            AdmissionOutcome::New(attempt) => Ok(attempt),
            AdmissionOutcome::Existing(status) => {
                Err(format!("expected fresh managed attempt, got {status:?}").into())
            }
        }
    }

    fn run(record: bool, fail: bool, accept: bool) -> Result<Run, Box<dyn Error>> {
        let (_directory, catalog, _ingress) = managed_adoption_fixture()?;
        let attempt = managed_attempt(&catalog)?;
        let leaf = object(153, vec![]);
        let objects = [leaf.clone(), object(154, vec![leaf.reference])];
        let Observation {
            mut budget,
            clock,
            events,
            root,
        } = request(record, accept)?;
        catalog.reserve_managed(ObjectInstallScope::Attempt(attempt), &objects, &mut budget)?;
        if fail {
            let guard = catalog.lock().map_err(|error| error.to_string())?;
            guard.execute_batch("CREATE TEMP TRIGGER reject_managed_registration BEFORE INSERT ON native_object_v1 BEGIN SELECT RAISE(ABORT,'injected registration refusal'); END")?;
            drop(guard);
        }
        let result =
            catalog.confirm_managed(ObjectInstallScope::Attempt(attempt), &objects, &mut budget);
        if let Some(root) = root {
            let _finished = root.finish(DiagnosticTerminalV1::from_result(&result));
        }
        let rows = counts(&catalog, &objects)?;
        let events = events
            .rows
            .lock()
            .map_err(|error| error.to_string())?
            .clone();
        Ok(Run {
            result,
            usage: budget.usage(),
            clocks: clock.0.load(Ordering::Relaxed),
            events,
            rows,
        })
    }

    fn require_phases(run: &Run, expected: &[u16], failure: Option<u16>) -> TestResult {
        let mut observed = Vec::new();
        for event in &run.events {
            if let DiagnosticEventKindV1::SpanBegin {
                span_id,
                stage: DiagnosticStageV1::SqliteWork,
                lane,
                ..
            } = event.kind
            {
                observed.push(lane.0);
                let terminal = if failure == Some(lane.0) {
                    DiagnosticTerminalV1::Error { code: 1 }
                } else {
                    DiagnosticTerminalV1::Ok
                };
                let closed = run.events.iter().filter(|event| matches!(event.kind,
                    DiagnosticEventKindV1::SpanEnd { span_id: id, outcome } if id == span_id && outcome == terminal
                )).count();
                check_eq!(closed, 1);
            }
        }
        check_eq!(observed.as_slice(), expected);
        Ok(())
    }

    #[test]
    fn managed_attempt_phase_trace_preserves_off_work_and_exact_registration() -> TestResult {
        let off = run(false, false, true)?;
        let record = run(true, false, true)?;
        check_eq!(off.result, Ok(()));
        check_eq!(record.result, off.result);
        check_eq!(record.usage, off.usage);
        check_eq!(record.rows, [2, 0, 2]);
        check_eq!(record.rows, off.rows);
        check_eq!(off.clocks, 0);
        check!(off.events.is_empty());
        check!(record.clocks > 0);
        require_phases(&record, &[9, 10, 4, 5, 3, 9, 10, 6, 7, 8, 3], None)
    }

    #[test]
    fn managed_attempt_phase_failure_closes_span_and_preserves_rollback() -> TestResult {
        let off = run(false, true, true)?;
        let record = run(true, true, true)?;
        let dropped = run(true, true, false)?;
        check!(
            matches!(&off.result, Err(IndexError::Storage(message)) if message.contains("injected registration refusal"))
        );
        check_eq!(record.result, off.result);
        check_eq!(dropped.result, off.result);
        check_eq!(record.usage, off.usage);
        check_eq!(dropped.usage, off.usage);
        check_eq!(record.rows, [0, 2, 0]);
        check_eq!(record.rows, off.rows);
        check_eq!(dropped.rows, off.rows);
        check_eq!(off.clocks, 0);
        check!(off.events.is_empty());
        check!(dropped.events.is_empty());
        require_phases(&record, &[9, 10, 4, 5, 3, 9, 10, 6, 7], Some(7))
    }

    static PROTECTION_STATEMENTS: AtomicU64 = AtomicU64::new(0);
    static LIFECYCLE_UPDATES: AtomicU64 = AtomicU64::new(0);
    static CONFIRM_QUEUE_WRITES: AtomicU64 = AtomicU64::new(0);

    fn observe_lifecycle_update(event: rusqlite::trace::TraceEvent<'_>) {
        if let rusqlite::trace::TraceEvent::Profile(statement, _duration) = event {
            let sql = statement.sql();
            if sql.starts_with("UPDATE native_gc_object_v1 SET") {
                let _prior = LIFECYCLE_UPDATES.fetch_add(1, Ordering::SeqCst);
            }
            if sql.starts_with("DELETE FROM native_gc_queue_v1")
                || sql.starts_with("INSERT INTO native_gc_queue_v1")
            {
                let _prior = CONFIRM_QUEUE_WRITES.fetch_add(1, Ordering::SeqCst);
            }
        }
    }

    fn observe_protection(event: rusqlite::trace::TraceEvent<'_>) {
        if let rusqlite::trace::TraceEvent::Profile(statement, _duration) = event
            && statement
                .sql()
                .starts_with("INSERT OR IGNORE INTO native_attempt_object_v1")
        {
            let _prior = PROTECTION_STATEMENTS.fetch_add(1, Ordering::SeqCst);
        }
    }

    #[test]
    fn managed_protection_batches_preserve_exact_replay_and_late_failure_rollback() -> TestResult {
        let (_directory, catalog, _ingress) = managed_adoption_fixture()?;
        let attempt = managed_attempt(&catalog)?;
        let objects: Vec<_> = (64..=128).map(|id| object(id, vec![])).collect();
        let scope = ObjectInstallScope::Attempt(attempt);
        let mut budget = admission_budget();
        catalog.reserve_managed(scope, &objects, &mut budget)?;
        for _replay in 0..2 {
            PROTECTION_STATEMENTS.store(0, Ordering::SeqCst);
            {
                let guard = catalog.lock().map_err(|error| error.to_string())?;
                guard.trace_v2(
                    rusqlite::trace::TraceEventCodes::SQLITE_TRACE_PROFILE,
                    Some(observe_protection),
                );
            }
            let result = catalog.confirm_managed(scope, &objects, &mut budget);
            {
                let guard = catalog.lock().map_err(|error| error.to_string())?;
                guard.trace_v2(rusqlite::trace::TraceEventCodes::empty(), None);
            }
            result?;
            check_eq!(PROTECTION_STATEMENTS.load(Ordering::SeqCst), 65);
            let guard = catalog.lock().map_err(|error| error.to_string())?;
            let operation = operation_key(attempt.operation)?;
            let fence = int(attempt.fence)?;
            for object in &objects {
                let key = object_key(object.reference);
                let stored: Vec<u8> = guard.query_row(
                    "SELECT row_digest FROM native_attempt_object_v1 WHERE operation=?1 AND fence=?2 AND object=?3",
                    params![&operation, fence, &key],
                    |row| row.get(0),
                )?;
                check_eq!(
                    stored,
                    fields_digest(b"qidx.native.attemptobject.v1", &(&operation, fence, &key))?
                );
                check!(object_install::confirmed_matches(&guard, object)?);
            }
            drop(guard);
        }

        let (_directory, catalog, _ingress) = managed_adoption_fixture()?;
        let attempt = managed_attempt(&catalog)?;
        let scope = ObjectInstallScope::Attempt(attempt);
        let mut budget = admission_budget();
        catalog.reserve_managed(scope, &objects, &mut budget)?;
        {
            let guard = catalog.lock().map_err(|error| error.to_string())?;
            guard.execute_batch(
                "CREATE TEMP TRIGGER reject_last_protection BEFORE INSERT ON native_attempt_object_v1
                 WHEN NEW.object = X'8080808080808080808080808080808080808080808080808080808080808080'
                 BEGIN SELECT RAISE(ABORT,'injected final protection refusal'); END",
            )?;
        }
        check!(matches!(
            catalog.confirm_managed(scope, &objects, &mut budget),
            Err(IndexError::Storage(message)) if message.contains("injected final protection refusal")
        ));
        let guard = catalog.lock().map_err(|error| error.to_string())?;
        for object in &objects {
            check!(!object_install::registered_matches(&guard, object)?);
            object_install::require_reserved(&guard, object)?;
            let protected: i64 = guard.query_row(
                "SELECT COUNT(*) FROM native_attempt_object_v1 WHERE operation=?1 AND fence=?2 AND object=?3",
                params![operation_key(attempt.operation)?, int(attempt.fence)?, object_key(object.reference)],
                |row| row.get(0),
            )?;
            check_eq!(protected, 0);
        }
        drop(guard);
        Ok(())
    }

    #[test]
    fn managed_reserve_new_keeps_child_first_replay_across_reopen() -> TestResult {
        let (directory, catalog, _ingress) = managed_adoption_fixture()?;
        let attempt = managed_attempt(&catalog)?;
        let leaf = object(201, vec![]);
        let parent = object(202, vec![leaf.reference]);
        let objects = [leaf.clone(), parent.clone()];
        let scope = ObjectInstallScope::Attempt(attempt);
        let mut budget = admission_budget();
        catalog.reserve_managed(scope, &objects, &mut budget)?;
        catalog.reserve_managed(scope, &objects, &mut budget)?;
        {
            let guard = catalog.lock().map_err(|error| error.to_string())?;
            for object in &objects {
                object_install::require_reserved(&guard, object)?;
            }
            let edges: i64 = guard.query_row(
                "SELECT COUNT(*) FROM native_object_install_edge_v1 WHERE parent=?1 AND child=?2",
                params![object_key(parent.reference), object_key(leaf.reference)],
                |row| row.get(0),
            )?;
            drop(guard);
            check_eq!(edges, 1);
        }
        LIFECYCLE_UPDATES.store(0, Ordering::SeqCst);
        CONFIRM_QUEUE_WRITES.store(0, Ordering::SeqCst);
        {
            let guard = catalog.lock().map_err(|error| error.to_string())?;
            guard.trace_v2(
                rusqlite::trace::TraceEventCodes::SQLITE_TRACE_PROFILE,
                Some(observe_lifecycle_update),
            );
        }
        let first_confirmation = catalog.confirm_managed(scope, &objects, &mut budget);
        {
            let guard = catalog.lock().map_err(|error| error.to_string())?;
            guard.trace_v2(rusqlite::trace::TraceEventCodes::empty(), None);
        }
        first_confirmation?;
        check_eq!(LIFECYCLE_UPDATES.load(Ordering::SeqCst), 2);
        check_eq!(CONFIRM_QUEUE_WRITES.load(Ordering::SeqCst), 0);
        drop(catalog);

        let reopened = open(directory.path())?;
        reopened.reserve_managed(scope, &objects, &mut budget)?;
        LIFECYCLE_UPDATES.store(0, Ordering::SeqCst);
        CONFIRM_QUEUE_WRITES.store(0, Ordering::SeqCst);
        {
            let guard = reopened.lock().map_err(|error| error.to_string())?;
            guard.trace_v2(
                rusqlite::trace::TraceEventCodes::SQLITE_TRACE_PROFILE,
                Some(observe_lifecycle_update),
            );
        }
        let replay_confirmation = reopened.confirm_managed(scope, &objects, &mut budget);
        {
            let guard = reopened.lock().map_err(|error| error.to_string())?;
            guard.trace_v2(rusqlite::trace::TraceEventCodes::empty(), None);
        }
        replay_confirmation?;
        check_eq!(LIFECYCLE_UPDATES.load(Ordering::SeqCst), 0);
        check_eq!(CONFIRM_QUEUE_WRITES.load(Ordering::SeqCst), 0);
        let guard = reopened.lock().map_err(|error| error.to_string())?;
        for object in &objects {
            check!(object_install::confirmed_matches(&guard, object)?);
        }
        let registered_edges: i64 = guard.query_row(
            "SELECT COUNT(*) FROM native_object_edge_v1 WHERE parent=?1 AND child=?2",
            params![object_key(parent.reference), object_key(leaf.reference)],
            |row| row.get(0),
        )?;
        check_eq!(registered_edges, 1);
        let provisional: i64 = guard.query_row(
            "SELECT COUNT(*) FROM native_object_install_v1 WHERE object IN (?1,?2)",
            params![object_key(leaf.reference), object_key(parent.reference)],
            |row| row.get(0),
        )?;
        let queued: i64 = guard.query_row(
            "SELECT COUNT(*) FROM native_gc_queue_v1 WHERE object IN (?1,?2)",
            params![object_key(leaf.reference), object_key(parent.reference)],
            |row| row.get(0),
        )?;
        drop(guard);
        check_eq!(provisional, 0);
        check_eq!(queued, 2);
        Ok(())
    }

    #[test]
    fn managed_confirm_replay_still_refuses_corrupt_or_retiring_lifecycle() -> TestResult {
        let (_directory, catalog, _ingress) = managed_adoption_fixture()?;
        let attempt = managed_attempt(&catalog)?;
        let object = object(216, vec![]);
        let scope = ObjectInstallScope::Attempt(attempt);
        let mut budget = admission_budget();
        catalog.reserve_managed(scope, std::slice::from_ref(&object), &mut budget)?;
        catalog.confirm_managed(scope, std::slice::from_ref(&object), &mut budget)?;

        let key = object_key(object.reference);
        let (generation, kind): (i64, i64) = {
            let guard = catalog.lock().map_err(|error| error.to_string())?;
            guard.query_row(
                "SELECT generation,kind FROM native_gc_object_v1 WHERE object=?1",
                params![&key],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )?
        };
        check_eq!(kind, 1);
        {
            let guard = catalog.lock().map_err(|error| error.to_string())?;
            check_eq!(
                guard.execute(
                    "UPDATE native_gc_object_v1 SET row_digest=zeroblob(32) WHERE object=?1",
                    params![&key],
                )?,
                1
            );
        }
        check!(matches!(
            catalog.confirm_managed(scope, std::slice::from_ref(&object), &mut budget),
            Err(IndexError::Storage(_))
        ));

        let retiring_digest = fields_digest(
            b"qidx.native.gc.object.v1",
            &(
                &key,
                generation,
                kind,
                1_i64,
                0_i64,
                0_i64,
                Option::<Vec<u8>>::None,
            ),
        )?;
        {
            let guard = catalog.lock().map_err(|error| error.to_string())?;
            check_eq!(
                guard.execute(
                    "UPDATE native_gc_object_v1 SET phase=1,row_digest=?2 WHERE object=?1",
                    params![&key, &retiring_digest],
                )?,
                1
            );
            check_eq!(
                reclaim::checked_live_object(&guard, object.reference, &mut budget)?,
                false
            );
        }
        check!(matches!(
            catalog.confirm_managed(scope, std::slice::from_ref(&object), &mut budget),
            Err(IndexError::Conflict)
        ));
        let guard = catalog.lock().map_err(|error| error.to_string())?;
        let queued: i64 = guard.query_row(
            "SELECT COUNT(*) FROM native_gc_queue_v1 WHERE object=?1",
            params![&key],
            |row| row.get(0),
        )?;
        check_eq!(queued, 1);
        Ok(())
    }

    // Independent reference for the original statement-only ABORT policy.
    // Production always consumes object_install::RESERVE_NEW_SQL.
    const ORIGINAL_RESERVE_NEW_SQL: &str =
        "INSERT INTO native_object_install_v1(object,payload,row_digest)
         SELECT ?1,?2,?3 WHERE NOT EXISTS(SELECT 1 FROM native_object_v1 WHERE object=?1)
         ON CONFLICT(object) DO NOTHING";

    fn constraint_rollback_probe(sql_text: &str) -> Result<(i32, bool), Box<dyn Error>> {
        let (directory, catalog, _ingress) = managed_adoption_fixture()?;
        let attempt = managed_attempt(&catalog)?;
        let prefix = object(203, vec![]);
        let mut guard = catalog.lock().map_err(|error| error.to_string())?;
        let tx = begin(&mut guard)?;
        let counter_before: (i64, Vec<u8>) = tx.query_row(
            "SELECT next,row_digest FROM native_counter_v1 WHERE name='attempt_fence'",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )?;
        object_install::reserve(&tx, &prefix)?;
        let payload = encode(&prefix)?;
        check!(try_insert_object_intent(&tx, attempt, &prefix, &payload)?);
        check!(reclaim::reserve_and_enqueue(&tx, prefix.reference)?);
        let late = object(204, vec![]);
        let error = tx
            .execute(
                sql_text,
                params![
                    object_key(late.reference),
                    rusqlite::types::Null,
                    vec![0_u8; 32]
                ],
            )
            .err()
            .ok_or("NULL payload unexpectedly passed the canonical reserve SQL")?;
        let rusqlite::Error::SqliteFailure(failure, _) = error else {
            return Err(format!("reserve constraint returned {error:?}").into());
        };
        let extended_code = failure.extended_code;
        let auto_rolled_back = tx.is_autocommit();
        drop(tx);
        drop(guard);
        drop(catalog);

        let reopened = open(directory.path())?;
        let guard = reopened.lock().map_err(|error| error.to_string())?;
        for table in [
            "native_object_install_v1",
            "native_object_intent_v1",
            "native_gc_object_v1",
            "native_gc_queue_v1",
        ] {
            let count: i64 = guard.query_row(
                &format!("SELECT COUNT(*) FROM {table} WHERE object=?1"),
                params![object_key(prefix.reference)],
                |row| row.get(0),
            )?;
            check_eq!(count, 0);
        }
        let counter_after: (i64, Vec<u8>) = guard.query_row(
            "SELECT next,row_digest FROM native_counter_v1 WHERE name='attempt_fence'",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )?;
        drop(guard);
        check_eq!(counter_after, counter_before);
        Ok((extended_code, auto_rolled_back))
    }

    #[test]
    fn reserve_new_constraint_rolls_back_entire_transaction_without_durable_prefix() -> TestResult {
        let original = constraint_rollback_probe(ORIGINAL_RESERVE_NEW_SQL)?;
        let canonical = constraint_rollback_probe(object_install::RESERVE_NEW_SQL)?;
        check_eq!(original.0, rusqlite::ffi::SQLITE_CONSTRAINT_NOTNULL);
        check_eq!(canonical.0, original.0);
        check!(!original.1);
        check!(canonical.1);
        Ok(())
    }

    fn confirmation_queue_rows(
        catalog: &SqliteCatalog,
        objects: &[RegisteredObject; 2],
    ) -> Result<Vec<(Vec<u8>, Vec<u8>)>, Box<dyn Error>> {
        let guard = catalog.lock().map_err(|error| error.to_string())?;
        let mut statement = guard.prepare(
            "SELECT object,row_digest FROM native_gc_queue_v1
             WHERE object IN (?1,?2) ORDER BY object",
        )?;
        let rows = statement.query_map(
            params![
                object_key(objects[0].reference),
                object_key(objects[1].reference)
            ],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    #[test]
    fn confirming_child_queue_is_authenticated_and_late_failure_rolls_back() -> TestResult {
        for ingress_scope in [false, true] {
            let (directory, catalog, ingress) = managed_adoption_fixture()?;
            let scope = if ingress_scope {
                ObjectInstallScope::Ingress(ingress)
            } else {
                ObjectInstallScope::Attempt(managed_attempt(&catalog)?)
            };
            let leaf = object(218, vec![]);
            let objects = [leaf.clone(), object(219, vec![leaf.reference])];
            let mut budget = admission_budget();
            catalog.reserve_managed(scope, &objects, &mut budget)?;
            {
                // Ingress reservation adopts the child and may remove its queue
                // row. Exercise an explicitly queued candidate in both scopes.
                let guard = catalog.lock().map_err(|error| error.to_string())?;
                reclaim::release(&guard, leaf.reference)?;
            }
            let gold = confirmation_queue_rows(&catalog, &objects)?;
            check_eq!(gold.len(), 2);
            for (key, checksum) in &gold {
                check_eq!(*checksum, fields_digest(b"qidx.native.gc.queue.v1", key)?);
            }
            {
                let guard = catalog.lock().map_err(|error| error.to_string())?;
                check_eq!(
                    guard.execute(
                        "UPDATE native_gc_queue_v1 SET row_digest=zeroblob(32) WHERE object=?1",
                        params![object_key(leaf.reference)],
                    )?,
                    1
                );
            }
            let corrupt = confirmation_queue_rows(&catalog, &objects)?;
            check!(matches!(
                catalog.confirm_managed(scope, &objects, &mut budget),
                Err(IndexError::Storage(message)) if message.contains("reclaim queue digest mismatch")
            ));
            check_eq!(counts(&catalog, &objects)?, [0, 2, 0]);
            check_eq!(confirmation_queue_rows(&catalog, &objects)?, corrupt);
            {
                let guard = catalog.lock().map_err(|error| error.to_string())?;
                let key = object_key(leaf.reference);
                check_eq!(
                    guard.execute(
                        "UPDATE native_gc_queue_v1 SET row_digest=?2 WHERE object=?1",
                        params![&key, fields_digest(b"qidx.native.gc.queue.v1", &key)?],
                    )?,
                    1
                );
                guard.execute_batch(
                    "CREATE TEMP TRIGGER reject_queue_parent BEFORE INSERT ON native_object_v1
                     WHEN NEW.object = X'DBDBDBDBDBDBDBDBDBDBDBDBDBDBDBDBDBDBDBDBDBDBDBDBDBDBDBDBDBDBDBDB'
                     BEGIN SELECT RAISE(ABORT,'injected queue parent refusal'); END",
                )?;
            }
            check!(matches!(
                catalog.confirm_managed(scope, &objects, &mut budget),
                Err(IndexError::Storage(message)) if message.contains("injected queue parent refusal")
            ));
            check_eq!(counts(&catalog, &objects)?, [0, 2, 0]);
            check_eq!(confirmation_queue_rows(&catalog, &objects)?, gold);
            {
                let guard = catalog.lock().map_err(|error| error.to_string())?;
                guard.execute_batch("DROP TRIGGER reject_queue_parent")?;
            }
            catalog.confirm_managed(scope, &objects, &mut budget)?;
            check_eq!(confirmation_queue_rows(&catalog, &objects)?, gold);
            drop(catalog);
            let reopened = open(directory.path())?;
            check_eq!(confirmation_queue_rows(&reopened, &objects)?, gold);
            let guard = reopened.lock().map_err(|error| error.to_string())?;
            for object in &objects {
                check!(object_install::confirmed_matches(&guard, object)?);
            }
            drop(guard);
        }
        Ok(())
    }

    #[test]
    fn confirming_child_without_queue_is_reenqueued_before_commit() -> TestResult {
        for ingress_scope in [false, true] {
            let (_directory, catalog, ingress) = managed_adoption_fixture()?;
            let scope = if ingress_scope {
                ObjectInstallScope::Ingress(ingress)
            } else {
                ObjectInstallScope::Attempt(managed_attempt(&catalog)?)
            };
            let leaf = object(220, vec![]);
            let objects = [leaf.clone(), object(221, vec![leaf.reference])];
            let mut budget = admission_budget();
            catalog.reserve_managed(scope, &objects, &mut budget)?;
            {
                let guard = catalog.lock().map_err(|error| error.to_string())?;
                reclaim::release(&guard, leaf.reference)?;
            }
            let gold = confirmation_queue_rows(&catalog, &objects)?;
            {
                let guard = catalog.lock().map_err(|error| error.to_string())?;
                check_eq!(
                    guard.execute(
                        "DELETE FROM native_gc_queue_v1 WHERE object=?1",
                        params![object_key(leaf.reference)],
                    )?,
                    1
                );
            }
            catalog.confirm_managed(scope, &objects, &mut budget)?;
            check_eq!(confirmation_queue_rows(&catalog, &objects)?, gold);
            let guard = catalog.lock().map_err(|error| error.to_string())?;
            for object in &objects {
                check!(object_install::confirmed_matches(&guard, object)?);
            }
            drop(guard);
        }
        Ok(())
    }
}
