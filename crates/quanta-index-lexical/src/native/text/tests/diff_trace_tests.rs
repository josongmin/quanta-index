use super::*;
use quanta_index_core::DiagnosticCounterV1;

#[derive(Debug, Default)]
struct DiffClock(AtomicU64);

impl DiagnosticClockV1 for DiffClock {
    fn now_ns(&self) -> u64 {
        self.0.fetch_add(1, Ordering::SeqCst)
    }

    fn thread_cpu_now_ns(&self) -> Option<u64> {
        Some(self.0.fetch_add(1, Ordering::SeqCst))
    }
}

#[derive(Debug, Default)]
struct DiffSink(Mutex<Vec<DiagnosticEmitV1>>);

impl DiagnosticEventSinkV1 for DiffSink {
    fn try_emit(&self, event: DiagnosticEmitV1) -> bool {
        let Ok(mut events) = self.0.try_lock() else {
            return false;
        };
        events.push(event);
        true
    }
}

// A port observation proves that the occurrence get receives the
// child budget, rather than only finding a sibling span in the output stream.
struct ObservedDiffObjects<'a> {
    original: &'a TestObjects,
    occurrence: ObjectRef,
}

impl ImmutableObjectPort for ObservedDiffObjects<'_> {
    fn identify(
        &self,
        payload: &[u8],
        children: &[ObjectRef],
        budget: &mut WorkBudget,
    ) -> Result<RegisteredObject, IndexError> {
        self.original.identify(payload, children, budget)
    }

    fn put(
        &self,
        payload: &[u8],
        children: &[ObjectRef],
        budget: &mut WorkBudget,
    ) -> Result<RegisteredObject, IndexError> {
        self.original.put(payload, children, budget)
    }

    fn install(
        &self,
        payload: &[u8],
        children: &[ObjectRef],
        sink: &dyn ObjectWriteSink,
        budget: &mut WorkBudget,
    ) -> Result<RegisteredObject, IndexError> {
        self.original.install(payload, children, sink, budget)
    }

    fn install_batch(
        &self,
        inputs: &[ObjectBatchInput],
        sink: &dyn ObjectWriteSink,
        budget: &mut WorkBudget,
    ) -> Result<quanta_index_core::index::RegisteredObjectBatch, IndexError> {
        self.original.install_batch(inputs, sink, budget)
    }

    fn get(
        &self,
        reference: ObjectRef,
        budget: &mut WorkBudget,
    ) -> Result<StoredObject, IndexError> {
        if reference == self.occurrence
            && let Some(diagnostic) = budget.request().diagnostic_context()
        {
            let marker = diagnostic
                .enter(DiagnosticStageV1::Operation, DiagnosticLaneV1(90))
                .map_err(|error| IndexError::BackendViolation(error.to_string()))?;
            marker
                .finish(DiagnosticTerminalV1::Ok)
                .map_err(|error| IndexError::BackendViolation(error.to_string()))?;
        }
        self.original.get(reference, budget)
    }
}

#[derive(Clone, Copy)]
enum DiffCase {
    Delete,
    CorruptOccurrence,
    CancelDuringRead,
    FlushWindow,
}

fn run_diff(
    fixture: &VisitorFixture,
    indexed_tokens: u64,
    diagnostic: Option<DiagnosticContextV1>,
) -> Result<(Result<(), IndexError>, WorkUsage), IndexError> {
    let request = RequestBudgetV1::for_duration(Duration::from_secs(30));
    let request = match diagnostic {
        Some(value) => request.with_diagnostic_context(value),
        None => request,
    };
    let mut work = WorkBudget::with_request(
        WorkLimits {
            reads: 10_000,
            writes: 10_000,
            bytes: 100_000_000,
            mutations: 10_000,
        },
        request,
    );
    let scratch = TestScratch::default();
    let custody = TestCustody;
    let spool = TextSpool::new(&scratch, &mut work)?;
    let objects = ObservedDiffObjects {
        original: &fixture.objects,
        occurrence: fixture.occurrence_ref,
    };
    let diagnostic = work.request().diagnostic_context().cloned();
    let mut context = AttemptContext {
        attempt: test_attempt(fixture.ticket.view.identity.workspace),
        objects: &objects,
        maps: &fixture.maps,
        custody: &custody,
        scratch: &scratch,
        budget: &mut work,
    };
    let prior = Contribution {
        record: fixture.source_ref,
        inventory: fixture.inventory_root.into(),
        indexed_tokens,
    };
    let mut posting_edits = PostingEdits::new()?;
    let mut confirmed = ConfirmedOccurrences::new(0)?;
    let result = observe_attempt(
        &mut context,
        diagnostic.as_ref(),
        DiagnosticStageV1::LexicalBuild,
        |_lexical, context| {
            diff_inventory(
                &owner_key(fixture.owner),
                Some(&prior),
                None,
                context,
                &spool,
                &mut posting_edits,
                &mut confirmed,
            )
        },
    );
    Ok((result, work.usage()))
}

fn assert_diff_trace(
    events: &[DiagnosticEmitV1],
    operation_id: quanta_index_core::DiagnosticSpanIdV1,
    case: DiffCase,
) -> Result<(), IndexError> {
    let mut starts = BTreeMap::new();
    let mut ends = BTreeMap::new();
    let mut cpu = BTreeMap::new();
    for event in events {
        match event.kind {
            DiagnosticEventKindV1::SpanBegin {
                span_id,
                parent_span_id,
                stage,
                lane,
                ..
            } => {
                check!(
                    starts
                        .insert(span_id, (parent_span_id, stage, lane))
                        .is_none()
                );
            }
            DiagnosticEventKindV1::SpanEnd { span_id, outcome } => {
                check!(ends.insert(span_id, outcome).is_none());
            }
            DiagnosticEventKindV1::Counter {
                span_id,
                key: DiagnosticCounterV1::ThreadCpuNs,
                value,
            } => {
                check!(cpu.insert(span_id, value).is_none());
            }
            DiagnosticEventKindV1::RequestIdentity { .. }
            | DiagnosticEventKindV1::PhaseIdentity { .. }
            | DiagnosticEventKindV1::Counter { .. }
            | DiagnosticEventKindV1::AnnRegionObservation { .. }
            | DiagnosticEventKindV1::SqliteSyncObservation { .. }
            | DiagnosticEventKindV1::OwnedRenewalRefusal { .. }
            | DiagnosticEventKindV1::SyncAggregate { .. }
            | DiagnosticEventKindV1::ObjectInstallAggregate { .. }
            | DiagnosticEventKindV1::ObjectKindAggregate { .. }
            | DiagnosticEventKindV1::AdoptionGroupAggregate { .. }
            | DiagnosticEventKindV1::Bm25CensusAggregate { .. }
            | DiagnosticEventKindV1::Bm25BoundaryAggregate { .. }
            | DiagnosticEventKindV1::Bm25LiveBuildAggregate { .. }
            | DiagnosticEventKindV1::Link { .. }
            | DiagnosticEventKindV1::Health { .. } => {}
        }
    }
    check_eq!(starts.len(), ends.len());
    check_eq!(
        starts
            .values()
            .filter(|(_, stage, _)| *stage == DiagnosticStageV1::LexicalBuild)
            .count(),
        1
    );
    check_eq!(
        starts
            .values()
            .filter(|(_, stage, _)| *stage == DiagnosticStageV1::NativeTextInventoryDiff)
            .count(),
        1
    );
    let lexical = starts
        .iter()
        .find(|(_, (_, stage, _))| *stage == DiagnosticStageV1::LexicalBuild)
        .map(|(id, _)| *id)
        .ok_or_else(|| IndexError::BackendViolation("missing lexical parent".into()))?;
    let diff = starts
        .iter()
        .find(|(_, (_, stage, _))| *stage == DiagnosticStageV1::NativeTextInventoryDiff)
        .map(|(id, _)| *id)
        .ok_or_else(|| IndexError::BackendViolation("missing inventory diff parent".into()))?;
    check_eq!(
        starts.get(&lexical).map(|value| value.0),
        Some(Some(operation_id))
    );
    check_eq!(
        starts.get(&diff),
        Some(&(
            Some(lexical),
            DiagnosticStageV1::NativeTextInventoryDiff,
            DiagnosticLaneV1(2)
        ))
    );
    let failure = matches!(
        case,
        DiffCase::CorruptOccurrence | DiffCase::CancelDuringRead
    );
    check_eq!(
        matches!(ends.get(&diff), Some(DiagnosticTerminalV1::Ok)),
        !failure
    );
    check_eq!(
        matches!(ends.get(&lexical), Some(DiagnosticTerminalV1::Ok)),
        !failure
    );
    check!(cpu.contains_key(&lexical));
    check!(cpu.contains_key(&diff));
    let mut inventory = 0_usize;
    let mut occurrence = 0_usize;
    let mut posting_push = 0_usize;
    let mut posting_flush = 0_usize;
    let mut port_reads = 0_usize;
    for (id, (parent, stage, lane)) in &starts {
        let expected = match (*stage, lane.0) {
            (DiagnosticStageV1::NativeTextInventoryRead, 30) => {
                inventory = inventory
                    .checked_add(1)
                    .ok_or(IndexError::ResourceLimited)?;
                Some(diff)
            }
            (DiagnosticStageV1::NativeTextOccurrenceRead, 31) => {
                occurrence = occurrence
                    .checked_add(1)
                    .ok_or(IndexError::ResourceLimited)?;
                Some(diff)
            }
            (DiagnosticStageV1::NativeTextPostingEdit, 32) => {
                posting_push = posting_push
                    .checked_add(1)
                    .ok_or(IndexError::ResourceLimited)?;
                Some(diff)
            }
            (DiagnosticStageV1::NativeTextPostingEdit, 33) => {
                posting_flush = posting_flush
                    .checked_add(1)
                    .ok_or(IndexError::ResourceLimited)?;
                Some(diff)
            }
            (DiagnosticStageV1::Operation, 90) => {
                port_reads = port_reads
                    .checked_add(1)
                    .ok_or(IndexError::ResourceLimited)?;
                let parent = parent.ok_or_else(|| {
                    IndexError::BackendViolation("occurrence get lacks parent".into())
                })?;
                check_eq!(
                    starts.get(&parent).map(|value| value.1),
                    Some(DiagnosticStageV1::NativeTextOccurrenceRead)
                );
                Some(parent)
            }
            _ => continue,
        };
        check_eq!(*parent, expected);
        check!(ends.contains_key(id));
        if *stage == DiagnosticStageV1::NativeTextOccurrenceRead {
            check_eq!(
                matches!(ends.get(id), Some(DiagnosticTerminalV1::Ok)),
                !failure
            );
        }
        if *stage != DiagnosticStageV1::Operation {
            check!(
                cpu.contains_key(id),
                "Diff child lacks CPU counter: {stage:?}"
            );
        }
    }
    check_eq!(
        inventory,
        if matches!(case, DiffCase::FlushWindow) {
            3
        } else {
            1
        }
    );
    // Row pushes are intentionally untraced. The 129-row fixture must produce
    // only three page spans and two complete SQL-flush spans.
    check_eq!(posting_push, 0);
    check!(occurrence >= 1);
    check!(port_reads >= 1);
    if failure {
        check_eq!(posting_push, 0);
        check_eq!(posting_flush, 0);
    } else if matches!(case, DiffCase::FlushWindow) {
        check_eq!(posting_flush, 2);
    } else {
        check_eq!(posting_flush, 0);
    }
    Ok(())
}

#[test]
fn diff_trace_children_preserve_results_work_and_object_port_parent() -> Result<(), IndexError> {
    for case in [
        DiffCase::Delete,
        DiffCase::CorruptOccurrence,
        DiffCase::CancelDuringRead,
        DiffCase::FlushWindow,
    ] {
        let mut fixture = visitor_fixture()?;
        fixture.objects.counted_occurrence = Some(fixture.occurrence_ref);
        let indexed_tokens = if matches!(case, DiffCase::FlushWindow) {
            for ordinal in 1..129 {
                fixture.maps.add(
                    fixture.inventory_root,
                    stored_term_key(&format!("term{ordinal:03}"), CaseMode::Sensitive)?,
                    fixture.occurrence_ref,
                )?;
            }
            258
        } else {
            2
        };
        if matches!(case, DiffCase::CorruptOccurrence) {
            fixture.objects.add(
                fixture.occurrence_ref,
                b"{\"positions\":[]}".to_vec(),
                Vec::new(),
            )?;
        }
        if matches!(case, DiffCase::CancelDuringRead) {
            fixture.objects.cancel_after_get = Some(fixture.occurrence_ref);
        }
        let (off, off_usage) = run_diff(&fixture, indexed_tokens, None)?;
        let sink = Arc::new(DiffSink::default());
        let trace = DiagnosticContextV1::new(
            DiagnosticCaptureIdV1([93; 16]),
            Arc::new(DiffClock::default()),
            sink.clone(),
        );
        let operation = trace
            .enter_root(DiagnosticRootKindV1::Operation, DiagnosticLaneV1(0))
            .map_err(|error| IndexError::BackendViolation(error.to_string()))?;
        let operation_id = operation.span_id();
        let (recorded, recorded_usage) = run_diff(
            &fixture,
            indexed_tokens,
            Some(trace.child_context(&operation)),
        )?;
        operation
            .finish(DiagnosticTerminalV1::Ok)
            .map_err(|error| IndexError::BackendViolation(error.to_string()))?;
        check_eq!(format!("{off:?}"), format!("{recorded:?}"));
        check_eq!(off_usage, recorded_usage);
        match case {
            DiffCase::Delete | DiffCase::FlushWindow => check!(off.is_ok()),
            DiffCase::CorruptOccurrence => {
                check!(matches!(off, Err(IndexError::CorruptObject(_))));
            }
            DiffCase::CancelDuringRead => check!(matches!(off, Err(IndexError::Cancelled))),
        }
        let events = sink
            .0
            .lock()
            .map_err(|error| IndexError::BackendViolation(error.to_string()))?;
        assert_diff_trace(&events, operation_id, case)?;
    }
    Ok(())
}

#[test]
fn text_step_none_uses_original_budget_without_clock_or_sink() -> Result<(), IndexError> {
    let fixture = visitor_fixture()?;
    let clock = Arc::new(DiffClock::default());
    let sink = Arc::new(DiffSink::default());
    let diagnostic =
        DiagnosticContextV1::new(DiagnosticCaptureIdV1([94; 16]), clock.clone(), sink.clone());
    let request =
        RequestBudgetV1::for_duration(Duration::from_secs(30)).with_diagnostic_context(diagnostic);
    let limits = WorkLimits {
        reads: 10,
        writes: 10,
        bytes: 1_000,
        mutations: 10,
    };
    let scratch = TestScratch::default();
    let custody = TestCustody;
    let attempt = test_attempt(fixture.ticket.view.identity.workspace);
    let mut direct_work = WorkBudget::with_request(limits, request.clone());
    let mut direct = AttemptContext {
        attempt,
        objects: &fixture.objects,
        maps: &fixture.maps,
        custody: &custody,
        scratch: &scratch,
        budget: &mut direct_work,
    };
    let direct_result = (|context: &mut AttemptContext<'_>| {
        check_eq!(context.attempt, attempt);
        check!(context.budget.request().diagnostic_context().is_some());
        context.budget.charge(WorkUsage {
            reads: 1,
            ..WorkUsage::default()
        })?;
        Ok::<AttemptRef, IndexError>(context.attempt)
    })(&mut direct)?;
    let mut observed_work = WorkBudget::with_request(limits, request);
    let mut observed = AttemptContext {
        attempt,
        objects: &fixture.objects,
        maps: &fixture.maps,
        custody: &custody,
        scratch: &scratch,
        budget: &mut observed_work,
    };
    let clock_before = clock.0.load(Ordering::SeqCst);
    let result = observe_text_step(
        None,
        DiagnosticStageV1::NativeTextOccurrenceRead,
        31,
        &mut observed,
        |context| {
            check_eq!(context.attempt, attempt);
            check!(context.budget.request().diagnostic_context().is_some());
            context.budget.charge(WorkUsage {
                reads: 1,
                ..WorkUsage::default()
            })?;
            Ok(context.attempt)
        },
    )?;
    check_eq!(result, direct_result);
    check_eq!(observed.attempt, attempt);
    check_eq!(observed_work.usage(), direct_work.usage());
    check_eq!(clock.0.load(Ordering::SeqCst), clock_before);
    check!(
        sink.0
            .lock()
            .map_err(|error| IndexError::BackendViolation(error.to_string()))?
            .is_empty()
    );
    Ok(())
}
