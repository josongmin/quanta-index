use std::sync::{Arc, Mutex};

use quanta_index_core::{
    DiagnosticCaptureIdV1, DiagnosticClockV1, DiagnosticContextV1, DiagnosticEmitV1,
    DiagnosticEventKindV1, DiagnosticEventSinkV1, DiagnosticLaneV1,
    DiagnosticOwnedRenewalRefusalReasonV1, DiagnosticOwnedRenewalRefusalV1, DiagnosticRootKindV1,
    DiagnosticStageV1, DiagnosticTerminalV1, index::ports::CatalogDiagnosticPort,
};

use super::*;

type CapturedRenewal = (
    Result<WorkClaim, IndexError>,
    Vec<DiagnosticOwnedRenewalRefusalV1>,
);
type RenewalFixture = (
    tempfile::TempDir,
    Arc<ManualClock>,
    SqliteCatalog,
    WorkIntent,
    OwnedWorkGrant,
);

#[derive(Debug)]
struct TraceClock(AtomicU64);

impl DiagnosticClockV1 for TraceClock {
    fn now_ns(&self) -> u64 {
        self.0.fetch_add(1, Ordering::Relaxed)
    }
}

#[derive(Debug, Default)]
struct TraceEvents(Mutex<Vec<DiagnosticEmitV1>>);

impl DiagnosticEventSinkV1 for TraceEvents {
    fn try_emit(&self, event: DiagnosticEmitV1) -> bool {
        let Ok(mut rows) = self.0.lock() else {
            return false;
        };
        rows.push(event);
        true
    }
}

#[test]
fn read_ticket_validation_trace_preserves_phase_order_and_off_clock() -> TestResult {
    let directory = tempfile::tempdir()?;
    let clock = Arc::new(ManualClock::new(2_000));
    let (catalog, genesis) = setup_with_clock(directory.path(), clock.clone())?;
    let session = catalog.open_read_session()?;
    let ticket = acquire_ticket_with_test_budget(
        &catalog,
        session.as_ref(),
        ViewSelection::Exact(genesis.identity),
        Duration::from_secs(30),
    )?;
    for observe in [false, true] {
        let events = Arc::new(TraceEvents::default());
        let trace_clock = Arc::new(TraceClock(AtomicU64::new(0)));
        let context = DiagnosticContextV1::new(
            DiagnosticCaptureIdV1([12; 16]),
            trace_clock.clone(),
            events.clone(),
        );
        let root = context.enter_root(DiagnosticRootKindV1::Operation, DiagnosticLaneV1(0))?;
        let child = context.child_context(&root);
        let mut request = quanta_index_core::RequestBudgetV1::for_duration(Duration::from_secs(30));
        if observe {
            request = request.with_diagnostic_context(child);
        }
        let work = WorkBudget::with_request(
            WorkLimits {
                reads: 1_000_000,
                writes: 1_000_000,
                bytes: 100_000_000,
                mutations: 1_000_000,
            },
            request,
        );
        for regressed in [false, true] {
            clock.set(if regressed { 1_999 } else { 2_000 });
            events
                .0
                .lock()
                .map_err(|error| std::io::Error::other(error.to_string()))?
                .clear();
            let before = trace_clock.0.load(Ordering::Relaxed);
            let result = catalog.validate_ticket_for_request(session.as_ref(), ticket.lease, &work);
            if regressed {
                check_eq!(
                    result,
                    Err(IndexError::Storage("catalog clock regressed".into()))
                );
            } else {
                result?;
            }
            let rows = events
                .0
                .lock()
                .map_err(|error| std::io::Error::other(error.to_string()))?;
            let phases: Vec<_> = rows
                .iter()
                .filter_map(|row| match row.kind {
                    DiagnosticEventKindV1::SpanBegin {
                        stage: DiagnosticStageV1::SqliteWork,
                        lane: DiagnosticLaneV1(lane),
                        ..
                    } => Some(lane),
                    _ => None,
                })
                .collect();
            if observe {
                let expected: &[u16] = if regressed {
                    &[200, 201, 202, 203, 204]
                } else {
                    &[200, 201, 202, 203, 204, 205]
                };
                check_eq!(phases.as_slice(), expected);
                let clock_span = rows.iter().find_map(|row| match row.kind {
                    DiagnosticEventKindV1::SpanBegin {
                        span_id,
                        stage: DiagnosticStageV1::SqliteWork,
                        lane: DiagnosticLaneV1(204),
                        ..
                    } => Some(span_id),
                    _ => None,
                });
                let terminal = if regressed {
                    DiagnosticTerminalV1::Error { code: 1 }
                } else {
                    DiagnosticTerminalV1::Ok
                };
                check!(rows.iter().any(|row| matches!(
                    row.kind,
                    DiagnosticEventKindV1::SpanEnd { span_id, outcome }
                        if Some(span_id) == clock_span && outcome == terminal
                )));
                check!(trace_clock.0.load(Ordering::Relaxed) > before);
            } else {
                check!(phases.is_empty());
                check_eq!(trace_clock.0.load(Ordering::Relaxed), before);
            }
        }
        root.finish(DiagnosticTerminalV1::Ok)?;
    }
    check_eq!(fixture_pin_count(&catalog)?, 1);
    clock.set(2_001);
    catalog.release_ticket(session.as_ref(), ticket.lease)?;
    Ok(())
}

fn captured_renewal(
    catalog: &SqliteCatalog,
    grant: &OwnedWorkGrant,
    capture: u8,
    observe: bool,
) -> Result<CapturedRenewal, Box<dyn Error>> {
    let events = Arc::new(TraceEvents::default());
    let context = DiagnosticContextV1::new(
        DiagnosticCaptureIdV1([capture; 16]),
        Arc::new(TraceClock(AtomicU64::new(0))),
        events.clone(),
    );
    let root = context.enter_root(DiagnosticRootKindV1::Operation, DiagnosticLaneV1(0))?;
    let root_child = context.child_context(&root);
    let renewal = root_child.enter(DiagnosticStageV1::SqliteWork, DiagnosticLaneV1(29))?;
    let renewal_id = renewal.span_id();
    let child = root_child.child_context(&renewal);
    let scope = catalog.enter_diagnostic_scope(observe.then_some(&child));
    let result = catalog.renew_owned_work(grant);
    drop(scope);
    renewal.finish(DiagnosticTerminalV1::from_result(&result))?;
    root.finish(DiagnosticTerminalV1::Ok)?;
    drop(child);
    drop(root_child);
    drop(context);
    let mut stored = events
        .0
        .lock()
        .map_err(|error| std::io::Error::other(error.to_string()))?;
    let rows = std::mem::take(&mut *stored);
    drop(stored);
    let mut observations = Vec::new();
    for row in &rows {
        if let DiagnosticEventKindV1::OwnedRenewalRefusal {
            span_id,
            observation,
        } = row.kind
        {
            if span_id != renewal_id {
                return Err("renewal refusal attached to the wrong span".into());
            }
            observations.push(observation);
        }
    }
    if !observe && rows.len() != 4 {
        return Err("Off renewal emitted Catalog diagnostic events".into());
    }
    Ok((result, observations))
}

fn renewal_fixture() -> Result<RenewalFixture, Box<dyn Error>> {
    let root = tempfile::tempdir()?;
    let clock = Arc::new(ManualClock::new(1_000));
    let (catalog, genesis) = setup_with_clock(root.path(), clock.clone())?;
    let intent = owner_work(93);
    let _published = publish_work_next(&catalog, &genesis, 1, &intent, 12_193, 93)?;
    let policy = OwnedWorkPolicy {
        dispatch_limits: WorkLimits {
            reads: 100,
            writes: 100,
            bytes: 4096,
            mutations: 100,
        },
        max_dispatches: 2,
        total_timeout_ms: 3_000,
        lease_ms: 900,
        retry_backoff_ms: 1,
    };
    let assignment = catalog
        .acquire_owned_work(&intent, policy, &admission_budget())?
        .ok_or("missing owned assignment")?;
    let grant = require_owned_grant(catalog.begin_owned_work(&assignment)?, "missing grant")?;
    Ok((root, clock, catalog, intent, grant))
}

#[test]
fn owned_renewal_diagnostic_only_on_actual_fence_loss() -> TestResult {
    let (_root, clock, catalog, intent, grant) = renewal_fixture()?;
    let (success, observations) = captured_renewal(&catalog, &grant, 1, true)?;
    let renewed = success?;
    check!(
        observations.is_empty(),
        "successful renewal emitted a refusal"
    );

    let before_work = catalog.work_status(&intent, &admission_budget())?;
    let before_owned = {
        let guard = catalog.lock()?;
        owned_work::read_owned(&guard, &intent)?.ok_or("missing owned row")?
    };
    let mut stale = grant.clone();
    stale.ordinal = stale.ordinal.checked_add(1).ok_or("ordinal overflow")?;
    let (result, observations) = captured_renewal(&catalog, &stale, 2, true)?;
    check_eq!(result, Err(IndexError::WorkFenceLost));
    check_eq!(
        observations,
        vec![DiagnosticOwnedRenewalRefusalV1 {
            reason: DiagnosticOwnedRenewalRefusalReasonV1::GrantMismatch,
            now_ms: 1_000,
            lease_until_ms: Some(renewed.lease_until_ms),
            deadline_ms: Some(grant.assignment.deadline_ms),
        }]
    );
    check_eq!(
        catalog.work_status(&intent, &admission_budget())?,
        before_work
    );
    let after_owned = {
        let guard = catalog.lock()?;
        owned_work::read_owned(&guard, &intent)?.ok_or("missing owned row")?
    };
    check_eq!(after_owned, before_owned);

    let (off_result, off_observations) = captured_renewal(&catalog, &stale, 3, false)?;
    check_eq!(off_result, Err(IndexError::WorkFenceLost));
    check!(off_observations.is_empty(), "Off emitted a refusal");
    check_eq!(
        catalog.work_status(&intent, &admission_budget())?,
        before_work
    );
    let mut wrong_fence = grant.clone();
    let UpdateKind::Materialize { claim } = &mut wrong_fence.assignment.request.kind else {
        return Err("owned grant lost its materialization claim".into());
    };
    claim.fence = claim.fence.checked_add(1).ok_or("fence overflow")?;
    for (now, capture) in [(1_000, 6), (renewed.lease_until_ms, 7)] {
        clock.set(now);
        let (result, observations) = captured_renewal(&catalog, &wrong_fence, capture, true)?;
        check_eq!(result, Err(IndexError::WorkFenceLost));
        check_eq!(
            observations,
            vec![DiagnosticOwnedRenewalRefusalV1 {
                reason: DiagnosticOwnedRenewalRefusalReasonV1::ClaimFence,
                now_ms: now,
                lease_until_ms: Some(renewed.lease_until_ms),
                deadline_ms: None,
            }]
        );
        check_eq!(
            catalog.work_status(&intent, &admission_budget())?,
            before_work
        );
        let held = {
            let guard = catalog.lock()?;
            owned_work::read_owned(&guard, &intent)?.ok_or("missing owned row")?
        };
        check_eq!(held, before_owned);
    }
    clock.set(renewed.lease_until_ms);
    let (expired, observations) = captured_renewal(&catalog, &grant, 4, true)?;
    check_eq!(expired, Err(IndexError::WorkFenceLost));
    check_eq!(
        observations,
        vec![DiagnosticOwnedRenewalRefusalV1 {
            reason: DiagnosticOwnedRenewalRefusalReasonV1::LeaseExpired,
            now_ms: renewed.lease_until_ms,
            lease_until_ms: Some(renewed.lease_until_ms),
            deadline_ms: None,
        }]
    );
    check_eq!(
        catalog.work_status(&intent, &admission_budget())?,
        before_work
    );
    Ok(())
}

#[test]
fn blocked_work_renewal_reports_not_claimed_without_mutating_state() -> TestResult {
    let (_root, _clock, catalog, intent, grant) = renewal_fixture()?;
    set_terminal_work_status(
        &catalog,
        &intent,
        &WorkStatus::Blocked(Refusal::ResourceLimited),
    )?;
    let before_work = catalog.work_status(&intent, &admission_budget())?;
    let before_owned = {
        let guard = catalog.lock()?;
        owned_work::read_owned(&guard, &intent)?.ok_or("missing owned row")?
    };
    let (result, observations) = captured_renewal(&catalog, &grant, 5, true)?;
    check_eq!(result, Err(IndexError::WorkFenceLost));
    check_eq!(
        observations,
        vec![DiagnosticOwnedRenewalRefusalV1 {
            reason: DiagnosticOwnedRenewalRefusalReasonV1::NotClaimed,
            now_ms: 1_000,
            lease_until_ms: None,
            deadline_ms: None,
        }]
    );
    check_eq!(
        catalog.work_status(&intent, &admission_budget())?,
        before_work
    );
    let after_owned = {
        let guard = catalog.lock()?;
        owned_work::read_owned(&guard, &intent)?.ok_or("missing owned row")?
    };
    check_eq!(after_owned, before_owned);
    Ok(())
}
