use super::*;

#[derive(Debug, Default)]
struct TraceClock(AtomicU64);

impl DiagnosticClockV1 for TraceClock {
    fn now_ns(&self) -> u64 {
        self.0.fetch_add(1, Ordering::SeqCst)
    }

    fn thread_cpu_now_ns(&self) -> Option<u64> {
        Some(self.0.fetch_add(1, Ordering::SeqCst))
    }
}

#[derive(Debug, Default)]
struct TraceSink(Mutex<Vec<DiagnosticEmitV1>>);

impl DiagnosticEventSinkV1 for TraceSink {
    fn try_emit(&self, event: DiagnosticEmitV1) -> bool {
        let Ok(mut events) = self.0.try_lock() else {
            return false;
        };
        events.push(event);
        true
    }
}

fn check_budget_context_restored(
    budget: &WorkBudget,
    expected: Option<&DiagnosticContextV1>,
) -> Result<(), IndexError> {
    let actual = budget.request().diagnostic_context();
    check_eq!(actual.is_some(), expected.is_some());
    if let (Some(actual), Some(expected)) = (actual, expected) {
        check!(actual.shares_capture_session(expected));
        check_eq!(actual.has_parent(), expected.has_parent());
    }
    Ok(())
}

fn run_change(
    fixture: &VisitorFixture,
    root: Root,
    change: &SourceChange,
    diagnostic: Option<DiagnosticContextV1>,
) -> Result<(Result<Root, IndexError>, WorkUsage), IndexError> {
    let request = RequestBudgetV1::for_duration(Duration::from_secs(30));
    let request = if let Some(diagnostic) = diagnostic {
        request.with_diagnostic_context(diagnostic)
    } else {
        request
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
    let diagnostic = work.request().diagnostic_context().cloned();
    let mut context = AttemptContext {
        attempt: test_attempt(fixture.ticket.view.identity.workspace),
        objects: &fixture.objects,
        maps: &fixture.maps,
        custody: &custody,
        scratch: &scratch,
        budget: &mut work,
    };
    let mut root = root;
    let result = observe_attempt(
        &mut context,
        diagnostic.as_ref(),
        DiagnosticStageV1::LexicalBuild,
        |_child, context| {
            let mut contributions = ContributionWrites::new()?;
            let mut inventories = InventoryReuse::new(fixture.recipe, 64)?;
            process_change(
                change,
                &mut root,
                fixture.recipe,
                context,
                &spool,
                &mut contributions,
                &mut inventories,
            )?;
            Ok(root)
        },
    );
    check_budget_context_restored(&work, diagnostic.as_ref())?;
    Ok((result, work.usage()))
}

fn run_bad_map_batch(
    fixture: &VisitorFixture,
    diagnostic: Option<DiagnosticContextV1>,
) -> Result<(Result<Root, IndexError>, WorkUsage), IndexError> {
    let request = RequestBudgetV1::for_duration(Duration::from_secs(30));
    let request = if let Some(diagnostic) = diagnostic {
        request.with_diagnostic_context(diagnostic)
    } else {
        request
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
    let diagnostic = work.request().diagnostic_context().cloned();
    let mut context = AttemptContext {
        attempt: test_attempt(fixture.ticket.view.identity.workspace),
        objects: &fixture.objects,
        maps: &fixture.maps,
        custody: &custody,
        scratch: &scratch,
        budget: &mut work,
    };
    let mut root = Root::default();
    let result = observe_attempt(
        &mut context,
        diagnostic.as_ref(),
        DiagnosticStageV1::LexicalBuild,
        |_child, context| {
            let mut batch = InventoryBatch::new(64)?;
            batch.rows.push(
                PendingTextChange {
                    owner: owner_key(fixture.owner),
                    prior_ref: None,
                    prior: None,
                    next: PendingTextNext::Ready,
                },
                context.budget,
            )?;
            batch.ready.push(ReadyTextNext::default(), context.budget)?;
            batch
                .bases
                .push(fixture.inventory_root.into(), context.budget)?;
            // The fixture base has exactly one term. A declared final count of
            // two is an independent cardinality corruption at map apply.
            batch.counts.push(2, context.budget)?;
            batch.attempt = Some(context.attempt);
            let mut contributions = ContributionWrites::new()?;
            let mut inventories = InventoryReuse::new(fixture.recipe, 64)?;
            batch.flush(
                &mut root,
                context,
                &spool,
                &mut contributions,
                &mut inventories,
            )?;
            Ok(root)
        },
    );
    check_budget_context_restored(&work, diagnostic.as_ref())?;
    Ok((result, work.usage()))
}

#[test]
fn inventory_trace_stages_preserve_work_and_close_under_lexical_parent() -> Result<(), IndexError> {
    let mut fixture = visitor_fixture()?;
    let prior = fixture_record(&fixture)?;
    let original_root: Root = decode(
        &fixture
            .objects
            .stored
            .get(&fixture.member.artifact)
            .ok_or_else(|| IndexError::CorruptObject("fixture root missing".into()))?
            .payload,
    )?;
    let empty_body = reference(42);
    let empty_record_ref = reference(43);
    fixture.objects.add(empty_body, Vec::new(), Vec::new())?;
    let mut empty_record = prior.clone();
    empty_record.bytes = empty_body;
    empty_record.byte_len = 0;
    fixture
        .objects
        .add(empty_record_ref, encode(&empty_record)?, vec![empty_body])?;

    let prepare_error = SourceChange {
        owner: fixture.owner,
        prior: None,
        next: Some((
            fixture.source_ref,
            SourceRecordOwner::new_unscoped(prior.clone()),
        )),
    };
    let empty_replacement = SourceChange {
        owner: fixture.owner,
        prior: Some((
            fixture.source_ref,
            SourceRecordOwner::new_unscoped(prior.clone()),
        )),
        next: Some((
            empty_record_ref,
            SourceRecordOwner::new_unscoped(empty_record),
        )),
    };
    let deletion = SourceChange {
        owner: fixture.owner,
        prior: Some((fixture.source_ref, SourceRecordOwner::new_unscoped(prior))),
        next: None,
    };

    let sink = Arc::new(TraceSink::default());
    let trace = DiagnosticContextV1::new(
        DiagnosticCaptureIdV1([77; 16]),
        Arc::new(TraceClock::default()),
        sink.clone(),
    );
    let operation = trace
        .enter_root(DiagnosticRootKindV1::Operation, DiagnosticLaneV1(0))
        .map_err(|error| IndexError::BackendViolation(error.to_string()))?;
    let child = trace.child_context(&operation);
    for (case, root, change, expected) in [
        ("prepare_error", Root::default(), &prepare_error, None),
        (
            "empty_replacement",
            original_root,
            &empty_replacement,
            Some((1_u64, 0_u64)),
        ),
        ("deletion", original_root, &deletion, Some((0_u64, 0_u64))),
    ] {
        let (off, off_usage) = run_change(&fixture, root, change, None)?;
        let (recorded, recorded_usage) = run_change(&fixture, root, change, Some(child.clone()))?;
        check!(
            off.is_ok() == expected.is_some(),
            "{case}: unexpected result: {off:?}"
        );
        if let Some((docs, tokens)) = expected {
            let observed = off
                .as_ref()
                .map_err(|error| IndexError::BackendViolation(format!("{case}: {error}")))?;
            check_eq!((observed.docs, observed.indexed_tokens), (docs, tokens));
        }
        check_eq!(format!("{off:?}"), format!("{recorded:?}"));
        check_eq!(off_usage, recorded_usage);
    }
    let (off, off_usage) = run_bad_map_batch(&fixture, None)?;
    let (recorded, recorded_usage) = run_bad_map_batch(&fixture, Some(child))?;
    check!(matches!(&off, Err(IndexError::CorruptObject(_))));
    check_eq!(format!("{off:?}"), format!("{recorded:?}"));
    check_eq!(off_usage, recorded_usage);
    let operation_id = operation.span_id();
    operation
        .finish(DiagnosticTerminalV1::Ok)
        .map_err(|error| IndexError::BackendViolation(error.to_string()))?;

    let events = sink
        .0
        .lock()
        .map_err(|error| IndexError::BackendViolation(error.to_string()))?;
    let mut parents = BTreeMap::new();
    let mut stages = BTreeMap::new();
    let mut lanes = BTreeMap::new();
    let mut terminals = BTreeMap::new();
    for event in events.iter() {
        if let DiagnosticEventKindV1::SpanBegin {
            span_id,
            parent_span_id,
            stage,
            lane,
            ..
        } = event.kind
        {
            check!(parents.insert(span_id, parent_span_id).is_none());
            check!(stages.insert(span_id, stage).is_none());
            check!(lanes.insert(span_id, lane).is_none());
        } else if let DiagnosticEventKindV1::SpanEnd { span_id, outcome } = event.kind {
            check!(terminals.insert(span_id, outcome).is_none());
        }
    }
    drop(events);
    for span_id in stages.keys() {
        check!(terminals.contains_key(span_id), "trace span lacks terminal");
    }
    for (stage, expected) in [
        (
            DiagnosticStageV1::NativeTextInventoryPrepare,
            vec![
                // File token preparation succeeds before the shared residual
                // install refuses at its own Prepare segment.
                DiagnosticTerminalV1::Ok,
                DiagnosticTerminalV1::Error { code: 1 },
                DiagnosticTerminalV1::Ok,
            ],
        ),
        (
            DiagnosticStageV1::NativeTextInventoryMapApply,
            vec![
                DiagnosticTerminalV1::Ok,
                DiagnosticTerminalV1::Error { code: 1 },
            ],
        ),
        (
            DiagnosticStageV1::NativeTextInventoryDiff,
            vec![DiagnosticTerminalV1::Ok, DiagnosticTerminalV1::Ok],
        ),
    ] {
        let observed = stages
            .iter()
            .filter(|(span_id, observed_stage)| {
                **observed_stage == stage && lanes.get(span_id) == Some(&DiagnosticLaneV1(2))
            })
            .map(|(span_id, _)| {
                let parent = parents.get(span_id).copied().flatten();
                let parent_stage = parent.and_then(|id| stages.get(&id)).copied();
                check_eq!(parent_stage, Some(DiagnosticStageV1::LexicalBuild));
                let grandparent = parent.and_then(|id| parents.get(&id)).copied().flatten();
                check_eq!(grandparent, Some(operation_id));
                terminals.get(span_id).copied().ok_or_else(|| {
                    IndexError::BackendViolation("inventory trace span did not close".into())
                })
            })
            .collect::<Result<Vec<_>, _>>()?;
        check_eq!(observed, expected);
    }
    let successful_prepare = stages
        .iter()
        .filter_map(|(span_id, stage)| {
            (*stage == DiagnosticStageV1::NativeTextInventoryPrepare
                && lanes.get(span_id) == Some(&DiagnosticLaneV1(2))
                && terminals.get(span_id) == Some(&DiagnosticTerminalV1::Ok))
            .then_some(*span_id)
        })
        .collect::<std::collections::BTreeSet<_>>();
    check_eq!(successful_prepare.len(), 2);
    for lane in [DiagnosticLaneV1(34), DiagnosticLaneV1(35)] {
        let mut owners = std::collections::BTreeSet::new();
        for (span_id, stage) in &stages {
            if *stage != DiagnosticStageV1::NativeTextInventoryPrepare
                || lanes.get(span_id) != Some(&lane)
            {
                continue;
            }
            let parent = parents.get(span_id).copied().flatten().ok_or_else(|| {
                IndexError::BackendViolation("inventory subphase lacks Prepare parent".into())
            })?;
            check!(successful_prepare.contains(&parent));
            check_eq!(terminals.get(span_id), Some(&DiagnosticTerminalV1::Ok));
            check!(owners.insert(parent), "duplicate inventory subphase owner");
        }
        check_eq!(owners, successful_prepare);
    }
    Ok(())
}

type ColdPrepareResult = Result<(u64, u64, bool, bool), IndexError>;
type ColdPrepareRun = (
    ColdPrepareResult,
    WorkUsage,
    Option<quanta_index_core::index::WorkBudgetDenial>,
    String,
);

fn run_cold_prepare(
    body: &str,
    byte_limit: u64,
    diagnostic: Option<DiagnosticContextV1>,
) -> Result<ColdPrepareRun, IndexError> {
    let fixture = visitor_fixture()?;
    let request = RequestBudgetV1::for_duration(Duration::from_secs(30));
    let request = if let Some(diagnostic) = diagnostic {
        request.with_diagnostic_context(diagnostic)
    } else {
        request
    };
    let mut work = WorkBudget::with_request(
        WorkLimits {
            reads: 100_000,
            writes: 100_000,
            bytes: byte_limit,
            mutations: 100_000,
        },
        request,
    );
    // This admits the optional token arena, then exercises the existing SQL
    // output fallback when the packed child credit is unavailable.
    let memory = Arc::new(TokenMemory {
        ceiling: 16 * 1024 * 1024,
        ..TokenMemory::default()
    });
    work.bind_memory_scope(Some(quanta_index_core::index::MemoryScope::new(Arc::new(
        TokenMemoryScope(memory.clone()),
    ))));
    let scratch = TestScratch::default();
    let custody = TestCustody;
    let spool = TextSpool::new(&scratch, &mut work)?;
    let mut edits = InventoryEdits::new(0)?;
    let mut writes = OccurrenceWrites::new();
    writes.pack_owners = true;
    let mut confirmed = ConfirmedOccurrences::new(0)?;
    let diagnostic = work.request().diagnostic_context().cloned();
    let mut context = AttemptContext {
        attempt: test_attempt(fixture.ticket.view.identity.workspace),
        objects: &fixture.objects,
        maps: &fixture.maps,
        custody: &custody,
        scratch: &scratch,
        budget: &mut work,
    };
    let result = prepare_inventory(
        body,
        &EntryMap::default(),
        0,
        &mut context,
        &spool,
        &mut confirmed,
        &mut edits,
        &mut writes,
    )
    .map(|prepared| {
        (
            prepared.indexed_tokens,
            prepared.term_count,
            prepared.changed,
            prepared.alias.is_some(),
        )
    });
    drop((context, writes, edits, confirmed, spool));
    check_eq!(memory.live.load(Ordering::Relaxed), 0);
    check_budget_context_restored(&work, diagnostic.as_ref())?;
    Ok((
        result,
        work.usage(),
        work.first_denial(),
        format!("{work:?}"),
    ))
}

#[test]
fn inventory_cold_prepare_work_lanes_preserve_off_record() -> Result<(), IndexError> {
    let body = "Alpha alpha β";
    let (off, off_usage, off_denial, _) = run_cold_prepare(body, 100_000_000, None)?;
    let sink = Arc::new(TraceSink::default());
    let trace = DiagnosticContextV1::new(
        DiagnosticCaptureIdV1([78; 16]),
        Arc::new(TraceClock::default()),
        sink.clone(),
    );
    let operation = trace
        .enter_root(DiagnosticRootKindV1::Operation, DiagnosticLaneV1(0))
        .map_err(|error| IndexError::BackendViolation(error.to_string()))?;
    let (recorded, recorded_usage, recorded_denial, _) =
        run_cold_prepare(body, 100_000_000, Some(trace.child_context(&operation)))?;
    operation
        .finish(DiagnosticTerminalV1::Ok)
        .map_err(|error| IndexError::BackendViolation(error.to_string()))?;

    check_eq!(off?, (3, 5, true, false));
    check_eq!(recorded?, (3, 5, true, false));
    check_eq!(recorded_usage, off_usage);
    check_eq!(off_denial, None);
    check_eq!(recorded_denial, None);

    let events = sink
        .0
        .lock()
        .map_err(|error| IndexError::BackendViolation(error.to_string()))?;
    let mut parents = BTreeMap::new();
    let mut lanes = BTreeMap::new();
    let mut stages = BTreeMap::new();
    let mut bytes = BTreeMap::new();
    let mut terminals = BTreeMap::new();
    for event in events.iter() {
        match event.kind {
            DiagnosticEventKindV1::SpanBegin {
                span_id,
                parent_span_id,
                stage,
                lane,
                ..
            } => {
                check!(parents.insert(span_id, parent_span_id).is_none());
                check!(stages.insert(span_id, stage).is_none());
                check!(lanes.insert(span_id, lane).is_none());
            }
            DiagnosticEventKindV1::Counter {
                span_id,
                key: quanta_index_core::DiagnosticCounterV1::WorkBytes,
                value,
            } => {
                check!(bytes.insert(span_id, value).is_none());
            }
            DiagnosticEventKindV1::SpanEnd { span_id, outcome } => {
                check!(terminals.insert(span_id, outcome).is_none());
            }
            _ => {}
        }
    }
    for (lane, parent_lane) in [(42, 34), (44, 34), (45, 34), (46, 35)] {
        let matches = stages
            .iter()
            .filter(|(span_id, stage)| {
                **stage == DiagnosticStageV1::NativeTextInventoryPrepare
                    && lanes.get(span_id) == Some(&DiagnosticLaneV1(lane))
            })
            .collect::<Vec<_>>();
        check_eq!(matches.len(), 1);
        let span_id = *matches[0].0;
        let parent =
            parents.get(&span_id).copied().flatten().ok_or_else(|| {
                IndexError::BackendViolation("cold Work lane lacks parent".into())
            })?;
        check_eq!(lanes.get(&parent), Some(&DiagnosticLaneV1(parent_lane)));
        let child_bytes = *bytes
            .get(&span_id)
            .ok_or_else(|| IndexError::BackendViolation("cold Work lane lacks WorkBytes".into()))?;
        check!(
            bytes
                .get(&parent)
                .is_some_and(|total| *total >= child_bytes)
        );
        check_eq!(terminals.get(&span_id), Some(&DiagnosticTerminalV1::Ok));
        if lane == 44 {
            check!(child_bytes > 0);
        }
    }
    check!(!stages.iter().any(|(span_id, stage)| {
        *stage == DiagnosticStageV1::NativeTextInventoryPrepare
            && lanes.get(span_id) == Some(&DiagnosticLaneV1(43))
    }));
    Ok(())
}

#[test]
fn inventory_trace_work_refusal_preserves_first_denied_charge() -> Result<(), IndexError> {
    let body = "Alpha alpha β";
    let (baseline, baseline_usage, baseline_denial, _) = run_cold_prepare(body, 100_000_000, None)?;
    check!(baseline.is_ok());
    check_eq!(baseline_denial, None);
    let limit = baseline_usage.bytes.checked_sub(1).ok_or_else(|| {
        IndexError::BackendViolation("cold fixture has no billed WorkBytes".into())
    })?;
    let (off, off_usage, off_denial, off_budget_state) = run_cold_prepare(body, limit, None)?;
    let sink = Arc::new(TraceSink::default());
    let trace = DiagnosticContextV1::new(
        DiagnosticCaptureIdV1([79; 16]),
        Arc::new(TraceClock::default()),
        sink.clone(),
    );
    let operation = trace
        .enter_root(DiagnosticRootKindV1::Operation, DiagnosticLaneV1(0))
        .map_err(|error| IndexError::BackendViolation(error.to_string()))?;
    let (recorded, recorded_usage, recorded_denial, recorded_budget_state) =
        run_cold_prepare(body, limit, Some(trace.child_context(&operation)))?;
    operation
        .finish(DiagnosticTerminalV1::Ok)
        .map_err(|error| IndexError::BackendViolation(error.to_string()))?;

    check!(matches!(&off, Err(IndexError::ResourceLimited)));
    check_eq!(format!("{off:?}"), format!("{recorded:?}"));
    check_eq!(
        off_denial,
        Some(quanta_index_core::index::WorkBudgetDenial::Bytes)
    );
    check_eq!(recorded_denial, off_denial);
    check_eq!(recorded_usage, off_usage);
    check!(off_usage.bytes <= limit);
    // WorkBudget debug includes the first denied charge's used, attempted,
    // limit, axis, and caller; equality detects a changed failure prefix.
    check!(off_budget_state.contains("denied_charge: Some"));
    check_eq!(recorded_budget_state, off_budget_state);
    let events = sink
        .0
        .lock()
        .map_err(|error| IndexError::BackendViolation(error.to_string()))?;
    for lane in [42, 44] {
        check!(events.iter().any(|event| matches!(
            event.kind,
            DiagnosticEventKindV1::SpanBegin {
                stage: DiagnosticStageV1::NativeTextInventoryPrepare,
                lane: DiagnosticLaneV1(observed),
                ..
            } if observed == lane
        )));
    }
    check!(!events.iter().any(|event| matches!(
        event.kind,
        DiagnosticEventKindV1::SpanBegin {
            stage: DiagnosticStageV1::NativeTextInventoryPrepare,
            lane: DiagnosticLaneV1(43),
            ..
        }
    )));
    Ok(())
}
