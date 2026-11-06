use super::*;
use quanta_index_core::DiagnosticCounterV1;

#[derive(Debug, Default)]
struct OccurrenceClock(AtomicU64);

impl DiagnosticClockV1 for OccurrenceClock {
    fn now_ns(&self) -> u64 {
        self.0.fetch_add(1, Ordering::SeqCst)
    }

    fn thread_cpu_now_ns(&self) -> Option<u64> {
        Some(self.0.fetch_add(1, Ordering::SeqCst))
    }
}

#[derive(Debug, Default)]
struct OccurrenceSink(Mutex<Vec<DiagnosticEmitV1>>);

impl DiagnosticEventSinkV1 for OccurrenceSink {
    fn try_emit(&self, event: DiagnosticEmitV1) -> bool {
        let Ok(mut events) = self.0.try_lock() else {
            return false;
        };
        events.push(event);
        true
    }
}

const OCCURRENCE_KEYS: [DiagnosticCounterV1; 6] = [
    DiagnosticCounterV1::TextOccurrencePriorValidationAttempts,
    DiagnosticCounterV1::TextOccurrenceNextValidationAttempts,
    DiagnosticCounterV1::TextOccurrenceRamReuses,
    DiagnosticCounterV1::TextOccurrenceSpillReuses,
    DiagnosticCounterV1::TextOccurrenceEqualReuses,
    DiagnosticCounterV1::TextOccurrenceValidationFailures,
];

fn occurrence_budget(diagnostic: Option<DiagnosticContextV1>) -> WorkBudget {
    let request = RequestBudgetV1::for_duration(Duration::from_secs(30));
    let request = if let Some(diagnostic) = diagnostic {
        request.with_diagnostic_context(diagnostic)
    } else {
        request
    };
    WorkBudget::with_request(
        WorkLimits {
            reads: 10_000,
            writes: 10_000,
            bytes: 100_000_000,
            mutations: 10_000,
        },
        request,
    )
}

type OccurrenceTrace = (
    Option<DiagnosticContextV1>,
    Option<quanta_index_core::DiagnosticSpanGuardV1>,
    Arc<OccurrenceClock>,
    Arc<OccurrenceSink>,
);

fn occurrence_trace(record: bool) -> Result<OccurrenceTrace, IndexError> {
    let clock = Arc::new(OccurrenceClock::default());
    let sink = Arc::new(OccurrenceSink::default());
    if !record {
        return Ok((None, None, clock, sink));
    }
    let trace =
        DiagnosticContextV1::new(DiagnosticCaptureIdV1([91; 16]), clock.clone(), sink.clone());
    let root = trace
        .enter_root(DiagnosticRootKindV1::Operation, DiagnosticLaneV1(0))
        .map_err(|error| IndexError::BackendViolation(error.to_string()))?;
    Ok((Some(trace.child_context(&root)), Some(root), clock, sink))
}

fn finish_occurrence_trace(
    root: Option<quanta_index_core::DiagnosticSpanGuardV1>,
    sink: &OccurrenceSink,
) -> Result<Vec<DiagnosticEmitV1>, IndexError> {
    if let Some(root) = root {
        root.finish(DiagnosticTerminalV1::Ok)
            .map_err(|error| IndexError::BackendViolation(error.to_string()))?;
    }
    Ok(sink
        .0
        .lock()
        .map_err(|error| IndexError::BackendViolation(error.to_string()))?
        .clone())
}

// Expected values are a literal external contract, independent of the producer's
// accumulator or the implementation's choice of helper functions.
fn check_occurrence_spans(
    events: &[DiagnosticEmitV1],
    expected: &[([u64; 6], DiagnosticTerminalV1)],
) -> Result<(), IndexError> {
    let mut spans = Vec::new();
    for (begin_index, event) in events.iter().enumerate() {
        if let DiagnosticEventKindV1::SpanBegin {
            span_id,
            stage: DiagnosticStageV1::NativeTextInventoryDiff,
            lane: DiagnosticLaneV1(2),
            ..
        } = event.kind
        {
            spans.push((begin_index, span_id));
        }
    }
    check_eq!(spans.len(), expected.len());
    let diff_ids: Vec<_> = spans.iter().map(|(_, span_id)| *span_id).collect();
    for ((begin_index, span_id), (values, outcome)) in spans.into_iter().zip(expected) {
        let ends: Vec<_> = events
            .iter()
            .enumerate()
            .filter_map(|(index, event)| {
                if let DiagnosticEventKindV1::SpanEnd {
                    span_id: ended,
                    outcome: actual,
                } = event.kind
                    && ended == span_id
                {
                    Some((index, actual))
                } else {
                    None
                }
            })
            .collect();
        check_eq!(ends.len(), 1);
        let (end_index, actual_outcome) = ends
            .first()
            .copied()
            .ok_or_else(|| IndexError::BackendViolation("Diff terminal absent".into()))?;
        check!(begin_index < end_index);
        check_eq!(actual_outcome, *outcome);
        let mut observed: [Option<u64>; 6] = [None; 6];
        for (event_index, event) in events.iter().enumerate() {
            if event_index <= begin_index || event_index >= end_index {
                continue;
            }
            if let DiagnosticEventKindV1::Counter {
                span_id: owner,
                key,
                value,
            } = event.kind
                && owner == span_id
                && let Some(index) = OCCURRENCE_KEYS.iter().position(|expected| *expected == key)
            {
                let slot = observed.get_mut(index).ok_or_else(|| {
                    IndexError::BackendViolation("occurrence counter slot absent".into())
                })?;
                check!(slot.replace(value).is_none());
            }
        }
        check_eq!(observed, (*values).map(Some));
        // An occurrence key may not escape to another span or appear after the
        // terminal. Generic Work counters on this span are intentionally ignored.
        for (index, event) in events.iter().enumerate() {
            if let DiagnosticEventKindV1::Counter {
                span_id: owner,
                key,
                ..
            } = event.kind
                && owner == span_id
                && OCCURRENCE_KEYS.contains(&key)
            {
                check!(index > begin_index && index < end_index);
            }
        }
    }
    for event in events {
        if let DiagnosticEventKindV1::Counter { span_id, key, .. } = event.kind
            && OCCURRENCE_KEYS.contains(&key)
        {
            check!(diff_ids.contains(&span_id));
        }
    }
    Ok(())
}

fn diff_once(
    owner: &[u8],
    prior: Option<&Contribution>,
    next: Option<&Contribution>,
    context: &mut AttemptContext<'_>,
    spool: &TextSpool,
    confirmed: &mut ConfirmedOccurrences,
) -> Result<(), IndexError> {
    let mut edits = PostingEdits::new()?;
    diff_inventory(owner, prior, next, context, spool, &mut edits, confirmed)
}

fn run_ram_and_prior_case(
    record: bool,
) -> Result<(Vec<DiagnosticEmitV1>, WorkUsage, usize, u64), IndexError> {
    let mut fixture = visitor_fixture()?;
    fixture.objects.counted_occurrence = Some(fixture.occurrence_ref);
    let second_inventory = MapRoot(Some(reference(51)));
    fixture.maps.add(
        second_inventory,
        stored_term_key("two", CaseMode::Sensitive)?,
        fixture.occurrence_ref,
    )?;
    let first = Contribution {
        record: fixture.source_ref,
        inventory: fixture.inventory_root.into(),
        indexed_tokens: 2,
    };
    let second = Contribution {
        inventory: second_inventory.into(),
        ..first
    };
    let second_owner = OwnerKey {
        stable_id: StableOwnerId([52; 16]),
        ..fixture.owner
    };
    let (diagnostic, root, clock, sink) = occurrence_trace(record)?;
    let mut work = occurrence_budget(diagnostic);
    let scratch = TestScratch::default();
    let custody = TestCustody;
    let spool = TextSpool::new(&scratch, &mut work)?;
    let attempt = test_attempt(fixture.ticket.view.identity.workspace);
    let mut confirmed = ConfirmedOccurrences::new(2)?;
    confirmed.prepare(attempt, &work)?;
    confirmed.remember(fixture.occurrence_ref, attempt, &work)?;
    {
        let mut context = AttemptContext {
            attempt,
            objects: &fixture.objects,
            maps: &fixture.maps,
            custody: &custody,
            scratch: &scratch,
            budget: &mut work,
        };
        diff_once(
            &owner_key(fixture.owner),
            None,
            Some(&first),
            &mut context,
            &spool,
            &mut confirmed,
        )?;
        diff_once(
            &owner_key(second_owner),
            None,
            Some(&second),
            &mut context,
            &spool,
            &mut confirmed,
        )?;
        diff_once(
            &owner_key(fixture.owner),
            Some(&first),
            None,
            &mut context,
            &spool,
            &mut confirmed,
        )?;
        diff_once(
            &owner_key(fixture.owner),
            Some(&first),
            Some(&first),
            &mut context,
            &spool,
            &mut confirmed,
        )?;
    }
    let usage = work.usage();
    let reads = fixture.objects.occurrence_reads.load(Ordering::Relaxed);
    let events = finish_occurrence_trace(root, &sink)?;
    Ok((events, usage, reads, clock.0.load(Ordering::SeqCst)))
}

#[test]
fn occurrence_trace_counts_ram_reuse_but_still_validates_prior_and_equal_identity()
-> Result<(), IndexError> {
    let (off_events, off_usage, off_reads, off_clock) = run_ram_and_prior_case(false)?;
    let (record_events, record_usage, record_reads, record_clock) = run_ram_and_prior_case(true)?;
    check!(off_events.is_empty());
    check_eq!(off_clock, 0);
    check!(record_clock > 0);
    check_eq!(off_usage, record_usage);
    check_eq!(off_reads, 3);
    check_eq!(record_reads, off_reads);
    check_occurrence_spans(
        &record_events,
        &[
            ([0, 1, 0, 0, 0, 0], DiagnosticTerminalV1::Ok),
            ([0, 0, 1, 0, 0, 0], DiagnosticTerminalV1::Ok),
            ([1, 0, 0, 0, 0, 0], DiagnosticTerminalV1::Ok),
            ([1, 0, 0, 0, 1, 0], DiagnosticTerminalV1::Ok),
        ],
    )
}

fn run_failed_validation_case(
    record: bool,
) -> Result<(Vec<DiagnosticEmitV1>, WorkUsage, usize, Vec<String>), IndexError> {
    let mut fixture = visitor_fixture()?;
    fixture.objects.counted_occurrence = Some(fixture.occurrence_ref);
    fixture.objects.add(
        fixture.occurrence_ref,
        b"{\"positions\":[-1]}".to_vec(),
        Vec::new(),
    )?;
    let next = Contribution {
        record: fixture.source_ref,
        inventory: fixture.inventory_root.into(),
        indexed_tokens: 2,
    };
    let (diagnostic, root, _clock, sink) = occurrence_trace(record)?;
    let mut work = occurrence_budget(diagnostic);
    let scratch = TestScratch::default();
    let custody = TestCustody;
    let spool = TextSpool::new(&scratch, &mut work)?;
    let attempt = test_attempt(fixture.ticket.view.identity.workspace);
    let mut confirmed = ConfirmedOccurrences::new(2)?;
    confirmed.prepare(attempt, &work)?;
    confirmed.remember(fixture.occurrence_ref, attempt, &work)?;
    let mut results = Vec::new();
    for _ in 0..2 {
        let mut context = AttemptContext {
            attempt,
            objects: &fixture.objects,
            maps: &fixture.maps,
            custody: &custody,
            scratch: &scratch,
            budget: &mut work,
        };
        let result = diff_once(
            &owner_key(fixture.owner),
            None,
            Some(&next),
            &mut context,
            &spool,
            &mut confirmed,
        );
        check!(matches!(&result, Err(IndexError::CorruptObject(_))));
        results.push(format!("{result:?}"));
    }
    fixture.objects.add(
        fixture.occurrence_ref,
        encode(&Occurrence {
            positions: Positions(vec![0, 2]),
        })?,
        Vec::new(),
    )?;
    let mut context = AttemptContext {
        attempt,
        objects: &fixture.objects,
        maps: &fixture.maps,
        custody: &custody,
        scratch: &scratch,
        budget: &mut work,
    };
    let recovered = diff_once(
        &owner_key(fixture.owner),
        None,
        Some(&next),
        &mut context,
        &spool,
        &mut confirmed,
    );
    check!(recovered.is_ok());
    results.push(format!("{recovered:?}"));
    let usage = work.usage();
    let reads = fixture.objects.occurrence_reads.load(Ordering::Relaxed);
    let events = finish_occurrence_trace(root, &sink)?;
    Ok((events, usage, reads, results))
}

#[test]
fn occurrence_trace_counts_failed_validations_without_memoizing_them() -> Result<(), IndexError> {
    let (off_events, off_usage, off_reads, off_results) = run_failed_validation_case(false)?;
    let (record_events, record_usage, record_reads, record_results) =
        run_failed_validation_case(true)?;
    check!(off_events.is_empty());
    check_eq!(off_usage, record_usage);
    check_eq!(off_reads, 3);
    check_eq!(record_reads, off_reads);
    check_eq!(record_results, off_results);
    check_occurrence_spans(
        &record_events,
        &[
            ([0, 1, 0, 0, 0, 1], DiagnosticTerminalV1::Error { code: 1 }),
            ([0, 1, 0, 0, 0, 1], DiagnosticTerminalV1::Error { code: 1 }),
            ([0, 1, 0, 0, 0, 0], DiagnosticTerminalV1::Ok),
        ],
    )
}

#[test]
fn occurrence_trace_distinguishes_spilled_reuse_from_ram_reuse() -> Result<(), IndexError> {
    let mut fixture = visitor_fixture()?;
    fixture.objects.counted_occurrence = Some(fixture.occurrence_ref);
    let next = Contribution {
        record: fixture.source_ref,
        inventory: fixture.inventory_root.into(),
        indexed_tokens: 2,
    };
    let (diagnostic, root, _clock, sink) = occurrence_trace(true)?;
    let mut work = occurrence_budget(diagnostic);
    let scratch = TestScratch::default();
    let custody = TestCustody;
    let spool = TextSpool::new(&scratch, &mut work)?;
    let attempt = test_attempt(fixture.ticket.view.identity.workspace);
    let mut confirmed = ConfirmedOccurrences::new(64)?;
    confirmed.prepare(attempt, &work)?;
    for byte in 100..164u8 {
        confirmed.remember(reference(byte), attempt, &work)?;
    }
    spool.admit_confirmed_occurrences(&[fixture.occurrence_ref], attempt, &mut work)?;
    spool.record_confirmed_occurrences(&[fixture.occurrence_ref], attempt, &work)?;
    let mut context = AttemptContext {
        attempt,
        objects: &fixture.objects,
        maps: &fixture.maps,
        custody: &custody,
        scratch: &scratch,
        budget: &mut work,
    };
    for _ in 0..2 {
        diff_once(
            &owner_key(fixture.owner),
            None,
            Some(&next),
            &mut context,
            &spool,
            &mut confirmed,
        )?;
    }
    check_eq!(fixture.objects.occurrence_reads.load(Ordering::Relaxed), 1);
    let events = finish_occurrence_trace(root, &sink)?;
    check_occurrence_spans(
        &events,
        &[
            ([0, 1, 0, 0, 0, 0], DiagnosticTerminalV1::Ok),
            ([0, 0, 0, 1, 0, 0], DiagnosticTerminalV1::Ok),
        ],
    )
}

#[test]
fn occurrence_trace_emits_six_zeros_for_empty_diff() -> Result<(), IndexError> {
    let fixture = visitor_fixture()?;
    let (diagnostic, root, _clock, sink) = occurrence_trace(true)?;
    let mut work = occurrence_budget(diagnostic);
    let scratch = TestScratch::default();
    let custody = TestCustody;
    let spool = TextSpool::new(&scratch, &mut work)?;
    let mut confirmed = ConfirmedOccurrences::new(0)?;
    let mut context = AttemptContext {
        attempt: test_attempt(fixture.ticket.view.identity.workspace),
        objects: &fixture.objects,
        maps: &fixture.maps,
        custody: &custody,
        scratch: &scratch,
        budget: &mut work,
    };
    diff_once(
        &owner_key(fixture.owner),
        None,
        None,
        &mut context,
        &spool,
        &mut confirmed,
    )?;
    let events = finish_occurrence_trace(root, &sink)?;
    check_occurrence_spans(&events, &[([0; 6], DiagnosticTerminalV1::Ok)])
}

#[test]
fn occurrence_trace_counts_corrupt_prior_payload_as_failed_validation() -> Result<(), IndexError> {
    let mut fixture = visitor_fixture()?;
    fixture.objects.counted_occurrence = Some(fixture.occurrence_ref);
    fixture.objects.add(
        fixture.occurrence_ref,
        b"{\"positions\":[-1]}".to_vec(),
        Vec::new(),
    )?;
    let prior = Contribution {
        record: fixture.source_ref,
        inventory: fixture.inventory_root.into(),
        indexed_tokens: 2,
    };
    let (diagnostic, root, _clock, sink) = occurrence_trace(true)?;
    let mut work = occurrence_budget(diagnostic);
    let scratch = TestScratch::default();
    let custody = TestCustody;
    let spool = TextSpool::new(&scratch, &mut work)?;
    let mut confirmed = ConfirmedOccurrences::new(0)?;
    let mut context = AttemptContext {
        attempt: test_attempt(fixture.ticket.view.identity.workspace),
        objects: &fixture.objects,
        maps: &fixture.maps,
        custody: &custody,
        scratch: &scratch,
        budget: &mut work,
    };
    check!(matches!(
        diff_once(
            &owner_key(fixture.owner),
            Some(&prior),
            None,
            &mut context,
            &spool,
            &mut confirmed,
        ),
        Err(IndexError::CorruptObject(_))
    ));
    check_eq!(fixture.objects.occurrence_reads.load(Ordering::Relaxed), 1);
    let events = finish_occurrence_trace(root, &sink)?;
    check_occurrence_spans(
        &events,
        &[([1, 0, 0, 0, 0, 1], DiagnosticTerminalV1::Error { code: 1 })],
    )
}

#[test]
fn occurrence_trace_excludes_prevalidation_term_and_attempt_refusals() -> Result<(), IndexError> {
    for invalid_term in [true, false] {
        let mut fixture = visitor_fixture()?;
        fixture.objects.counted_occurrence = Some(fixture.occurrence_ref);
        let inventory = if invalid_term {
            let root = MapRoot(Some(reference(98)));
            fixture.maps.add(root, vec![0], fixture.occurrence_ref)?;
            root
        } else {
            fixture.inventory_root
        };
        let next = Contribution {
            record: fixture.source_ref,
            inventory: inventory.into(),
            indexed_tokens: 2,
        };
        let (diagnostic, root, _clock, sink) = occurrence_trace(true)?;
        let mut work = occurrence_budget(diagnostic);
        let scratch = TestScratch::default();
        let custody = TestCustody;
        let spool = TextSpool::new(&scratch, &mut work)?;
        let attempt = test_attempt(fixture.ticket.view.identity.workspace);
        let mut confirmed = ConfirmedOccurrences::new(1)?;
        if !invalid_term {
            let foreign = AttemptRef {
                fence: attempt
                    .fence
                    .checked_add(1)
                    .ok_or(IndexError::ResourceLimited)?,
                ..attempt
            };
            confirmed.prepare(foreign, &work)?;
        }
        let mut context = AttemptContext {
            attempt,
            objects: &fixture.objects,
            maps: &fixture.maps,
            custody: &custody,
            scratch: &scratch,
            budget: &mut work,
        };
        let result = diff_once(
            &owner_key(fixture.owner),
            None,
            Some(&next),
            &mut context,
            &spool,
            &mut confirmed,
        );
        if invalid_term {
            check!(matches!(result, Err(IndexError::CorruptObject(_))));
        } else {
            check!(matches!(result, Err(IndexError::FenceLost)));
        }
        check_eq!(fixture.objects.occurrence_reads.load(Ordering::Relaxed), 0);
        let events = finish_occurrence_trace(root, &sink)?;
        check_occurrence_spans(
            &events,
            &[([0; 6], DiagnosticTerminalV1::Error { code: 1 })],
        )?;
    }
    Ok(())
}

#[test]
fn occurrence_trace_overflow_emits_health_without_partial_family() -> Result<(), IndexError> {
    let (diagnostic, root, _clock, sink) = occurrence_trace(true)?;
    let diagnostic = diagnostic.ok_or_else(|| {
        IndexError::BackendViolation("recording diagnostic context absent".into())
    })?;
    let span = diagnostic
        .enter(
            DiagnosticStageV1::NativeTextInventoryDiff,
            DiagnosticLaneV1(2),
        )
        .map_err(|error| IndexError::BackendViolation(error.to_string()))?;
    let mut totals = OccurrenceValidationTotals {
        prior_validation_attempts: u64::MAX,
        ..OccurrenceValidationTotals::default()
    };
    totals.attempt(ValidationSide::Prior);
    totals.emit(&diagnostic, &span);
    let diff_span_id = span.span_id();
    span.finish(DiagnosticTerminalV1::Ok)
        .map_err(|error| IndexError::BackendViolation(error.to_string()))?;
    let events = finish_occurrence_trace(root, &sink)?;
    let health: Vec<_> = events
        .iter()
        .enumerate()
        .filter_map(|(index, event)| {
            if event.kind
                == (DiagnosticEventKindV1::Health {
                    code: quanta_index_core::DiagnosticHealthCodeV1::CounterOverflow,
                })
            {
                Some(index)
            } else {
                None
            }
        })
        .collect();
    check_eq!(health.len(), 1);
    let diff_end = events
        .iter()
        .position(|event| {
            matches!(
                event.kind,
                DiagnosticEventKindV1::SpanEnd { span_id, .. } if span_id == diff_span_id
            )
        })
        .ok_or_else(|| IndexError::BackendViolation("Diff terminal absent".into()))?;
    check!(health.first().is_some_and(|index| *index < diff_end));
    check!(events.iter().all(|event| {
        !matches!(
            event.kind,
            DiagnosticEventKindV1::Counter { key, .. } if OCCURRENCE_KEYS.contains(&key)
        )
    }));
    Ok(())
}

fn run_unparented_diff(
    record: bool,
) -> Result<(String, WorkUsage, usize, Vec<DiagnosticEmitV1>), IndexError> {
    let mut fixture = visitor_fixture()?;
    fixture.objects.counted_occurrence = Some(fixture.occurrence_ref);
    let next = Contribution {
        record: fixture.source_ref,
        inventory: fixture.inventory_root.into(),
        indexed_tokens: 2,
    };
    let sink = Arc::new(OccurrenceSink::default());
    let (diagnostic, root) = if record {
        let trace = DiagnosticContextV1::new(
            DiagnosticCaptureIdV1([92; 16]),
            Arc::new(OccurrenceClock::default()),
            sink.clone(),
        );
        let root = trace
            .enter_root(DiagnosticRootKindV1::Operation, DiagnosticLaneV1(0))
            .map_err(|error| IndexError::BackendViolation(error.to_string()))?;
        // Deliberately retain the valid capture root's unparented context.
        // Diff admission fails; a subsequent child must not inherit it.
        (Some(trace), Some(root))
    } else {
        (None, None)
    };
    // Spool setup is outside the Diff trace contract. Bind the unparented
    // diagnostic only for the product operation under test.
    let mut work = occurrence_budget(None);
    let scratch = TestScratch::default();
    let custody = TestCustody;
    let spool = TextSpool::new(&scratch, &mut work)?;
    let mut confirmed = ConfirmedOccurrences::new(0)?;
    let result = {
        let mut diff_work = work.scoped_diagnostic_context(diagnostic.clone());
        let result = {
            let mut context = AttemptContext {
                attempt: test_attempt(fixture.ticket.view.identity.workspace),
                objects: &fixture.objects,
                maps: &fixture.maps,
                custody: &custody,
                scratch: &scratch,
                budget: &mut diff_work,
            };
            diff_once(
                &owner_key(fixture.owner),
                None,
                Some(&next),
                &mut context,
                &spool,
                &mut confirmed,
            )
        };
        let restored = diff_work.request().diagnostic_context();
        check_eq!(restored.is_some(), diagnostic.is_some());
        if let (Some(restored), Some(original)) = (restored, diagnostic.as_ref()) {
            check!(restored.shares_capture_session(original));
            check!(!restored.has_parent());
        }
        result
    };
    check!(work.request().diagnostic_context().is_none());
    let usage = work.usage();
    let reads = fixture.objects.occurrence_reads.load(Ordering::Relaxed);
    let events = finish_occurrence_trace(root, &sink)?;
    Ok((format!("{result:?}"), usage, reads, events))
}

#[test]
fn occurrence_trace_masks_children_when_diff_span_admission_fails() -> Result<(), IndexError> {
    let (off_result, off_usage, off_reads, off_events) = run_unparented_diff(false)?;
    let (record_result, record_usage, record_reads, record_events) = run_unparented_diff(true)?;
    check_eq!(off_result, "Ok(())");
    check_eq!(record_result, off_result);
    check_eq!(record_usage, off_usage);
    check_eq!(record_reads, off_reads);
    check_eq!(record_reads, 1);
    check!(off_events.is_empty());
    let roots = record_events
        .iter()
        .filter(|event| {
            matches!(
                event.kind,
                DiagnosticEventKindV1::SpanBegin {
                    stage: DiagnosticStageV1::Operation,
                    lane: DiagnosticLaneV1(0),
                    ..
                }
            )
        })
        .count();
    let all_begins = record_events
        .iter()
        .filter(|event| matches!(event.kind, DiagnosticEventKindV1::SpanBegin { .. }))
        .count();
    check_eq!(roots, 1);
    check_eq!(all_begins, 1);
    // Core's missing-parent guard and the fail-open adapter each record the
    // admission refusal once. More health events imply an unmasked child tried
    // to open beneath the context that has no parent.
    let missing_parent_health = record_events
        .iter()
        .filter(|event| {
            matches!(
                event.kind,
                DiagnosticEventKindV1::Health {
                    code: quanta_index_core::DiagnosticHealthCodeV1::MissingParentSpan
                }
            )
        })
        .count();
    check_eq!(missing_parent_health, 2);
    check!(record_events.iter().all(|event| {
        !matches!(
            event.kind,
            DiagnosticEventKindV1::Counter { key, .. } if OCCURRENCE_KEYS.contains(&key)
        )
    }));
    Ok(())
}
