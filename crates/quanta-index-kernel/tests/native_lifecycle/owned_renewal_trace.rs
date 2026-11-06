use super::*;

#[test]
fn owned_renewal_binds_catalog_trace_and_restores_outer_scope() -> TestResult {
    use quanta_index_core::diagnostic_trace::{
        DiagnosticCaptureIdV1, DiagnosticClockV1, DiagnosticContextV1, DiagnosticCounterV1,
        DiagnosticEmitV1, DiagnosticEventKindV1, DiagnosticEventSinkV1, DiagnosticLaneV1,
        DiagnosticRootKindV1, DiagnosticStageV1, DiagnosticTerminalV1,
    };
    use quanta_index_core::index::ports::{CatalogDiagnosticPort, WorkClaimPort};
    use std::sync::{
        Mutex,
        atomic::{AtomicU64, AtomicUsize, Ordering},
    };

    #[derive(Debug, Default)]
    struct Clock(AtomicU64);
    impl DiagnosticClockV1 for Clock {
        fn now_ns(&self) -> u64 {
            self.0.fetch_add(1, Ordering::Relaxed)
        }
    }
    #[derive(Debug, Default)]
    struct Sink(Mutex<Vec<DiagnosticEmitV1>>);
    impl DiagnosticEventSinkV1 for Sink {
        fn try_emit(&self, event: DiagnosticEmitV1) -> bool {
            let Ok(mut rows) = self.0.try_lock() else {
                return false;
            };
            rows.push(event);
            true
        }
    }
    impl Sink {
        fn rows(&self) -> TestResult<Vec<DiagnosticEmitV1>> {
            Ok(self
                .0
                .lock()
                .map_err(|error| format!("renewal trace lock poisoned: {error}"))?
                .clone())
        }
    }

    let directory = tempfile::tempdir()?;
    let (engine, objects, catalog) = owner_kernel(directory.path(), Arc::new(AtomicUsize::new(0)))?;
    let intent = staged_owned_job(&engine, &objects, &catalog)?;
    let assignment = engine
        .acquire_owned_work(&intent, owned_policy(), &budget())?
        .ok_or("missing owned assignment")?;
    let grant = require_owned_grant(engine.begin_owned_work(&assignment)?, "missing grant")?;
    let sink = Arc::new(Sink::default());
    let context = DiagnosticContextV1::new(
        DiagnosticCaptureIdV1([96; 16]),
        Arc::new(Clock::default()),
        sink.clone(),
    );
    let root = context.enter_root(DiagnosticRootKindV1::Operation, DiagnosticLaneV1(0))?;
    let outer_id = root.span_id();
    let outer = context.child_context(&root);
    let scope = catalog.enter_diagnostic_scope(Some(&outer));

    let success_span = outer.enter(DiagnosticStageV1::SqliteWork, DiagnosticLaneV1(29))?;
    let success_id = success_span.span_id();
    let success_child = outer.child_context(&success_span);
    let renewed = engine.renew_owned_work(&grant, Some(&success_child))?;
    assert!(renewed.lease_until_ms <= assignment.deadline_ms);
    success_span.finish(DiagnosticTerminalV1::Ok)?;

    let mut stale = grant.clone();
    stale.ordinal = stale.ordinal.checked_add(1).ok_or("ordinal overflow")?;
    let failure_span = outer.enter(DiagnosticStageV1::SqliteWork, DiagnosticLaneV1(29))?;
    let failure_id = failure_span.span_id();
    let failure_child = outer.child_context(&failure_span);
    assert_eq!(
        engine.renew_owned_work(&stale, Some(&failure_child)),
        Err(IndexError::WorkFenceLost),
    );
    failure_span.finish(DiagnosticTerminalV1::Error { code: 1 })?;

    let before_mask = sink.rows()?.len();
    let _off = engine.renew_owned_work(&grant, None)?;
    assert_eq!(
        sink.rows()?.len(),
        before_mask,
        "None must mask outer scope"
    );
    let _outer_renewed = catalog.renew_owned_work(&grant)?;
    drop(scope);
    let before_unbound = sink.rows()?.len();
    let _unbound = catalog.renew_owned_work(&grant)?;
    assert_eq!(
        sink.rows()?.len(),
        before_unbound,
        "scope must restore on return"
    );
    root.finish(DiagnosticTerminalV1::Ok)?;

    let rows = sink.rows()?;
    let lock_holds = rows
        .iter()
        .filter_map(|row| {
            if let DiagnosticEventKindV1::SpanBegin {
                span_id,
                parent_span_id: Some(parent),
                stage: DiagnosticStageV1::CatalogLockHold,
                ..
            } = row.kind
            {
                Some((span_id, parent))
            } else {
                None
            }
        })
        .collect::<Vec<_>>();
    assert!(lock_holds.iter().any(|(_, parent)| *parent == success_id));
    assert!(lock_holds.iter().any(|(_, parent)| *parent == failure_id));
    assert!(lock_holds.iter().any(|(_, parent)| *parent == outer_id));
    for (hold, parent) in lock_holds {
        if parent != success_id && parent != failure_id {
            continue;
        }
        let counter_at = rows.iter().position(|row| {
            matches!(row.kind,
            DiagnosticEventKindV1::Counter {
                span_id, key: DiagnosticCounterV1::SqliteCacheHits, ..
            } if span_id == hold)
        });
        let ended_at = rows.iter().position(|row| {
            matches!(row.kind,
            DiagnosticEventKindV1::SpanEnd { span_id, .. } if span_id == hold)
        });
        assert!(
            matches!((counter_at, ended_at), (Some(counter), Some(end)) if counter < end),
            "Catalog pager counter must precede lock-hold end"
        );
    }
    Ok(())
}
