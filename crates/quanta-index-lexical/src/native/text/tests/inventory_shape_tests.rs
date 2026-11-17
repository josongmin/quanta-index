use super::*;
use crate::native::text_occurrence::block::PackedBlockView as _;

#[derive(Debug, Default)]
struct ShapeClock(AtomicU64);

impl DiagnosticClockV1 for ShapeClock {
    fn now_ns(&self) -> u64 {
        self.0.fetch_add(1, Ordering::SeqCst)
    }
}

#[derive(Debug, Default)]
struct ShapeSink(Mutex<Vec<DiagnosticEmitV1>>);

impl DiagnosticEventSinkV1 for ShapeSink {
    fn try_emit(&self, event: DiagnosticEmitV1) -> bool {
        let Ok(mut events) = self.0.try_lock() else {
            return false;
        };
        events.push(event);
        true
    }
}

struct ShapeRun {
    indexed_tokens: u64,
    term_count: u64,
    changed: bool,
    has_alias: bool,
    pending: usize,
    sql_has_terms: bool,
    work: WorkUsage,
    counters: BTreeMap<String, u64>,
    census: Option<CensusTrace>,
    output_groups: Option<BTreeMap<Vec<u8>, Vec<u32>>>,
}

#[derive(Debug, PartialEq, Eq)]
struct CensusTrace {
    spans: usize,
    work_bytes: u64,
}

fn run_cold_shape(body: &str, record: bool) -> Result<ShapeRun, IndexError> {
    run_cold_shape_with_ceiling(body, record, 16 * 1024 * 1024, false)
}

fn staged_packed_groups(
    writes: &OccurrenceWrites<'_>,
) -> Result<BTreeMap<Vec<u8>, Vec<u32>>, IndexError> {
    check_eq!(writes.inputs.len(), 1);
    let input = writes.inputs.first().ok_or(IndexError::ResourceLimited)?;
    check!(input.children.is_empty());
    let payload_bytes =
        u64::try_from(input.payload.len()).map_err(|_error| IndexError::ResourceLimited)?;
    let object = StoredObject::new(
        RegisteredObject::unscoped(reference(91), payload_bytes, Vec::new()),
        input.payload.clone(),
        None,
    );
    let verify_budget = budget()?;
    let checked =
        crate::native::text_occurrence::block::CheckedBlock::parse(object, &verify_budget)?;
    let mut groups = BTreeMap::new();
    for ordinal in 0..checked.term_count() {
        let key = checked.key(ordinal)?.to_vec();
        let range = checked.selection(&key, &verify_budget)?;
        let encoded = checked
            .object()
            .payload
            .get(range)
            .ok_or(IndexError::ResourceLimited)?;
        let mut positions = Vec::new();
        let mut chunks = encoded.chunks_exact(4);
        for position in &mut chunks {
            positions.push(u32::from_be_bytes(
                position
                    .try_into()
                    .map_err(|_error| IndexError::ResourceLimited)?,
            ));
        }
        check!(chunks.remainder().is_empty());
        check!(groups.insert(key, positions).is_none());
    }
    Ok(groups)
}

fn run_cold_shape_with_ceiling(
    body: &str,
    record: bool,
    ceiling: u64,
    inspect_output: bool,
) -> Result<ShapeRun, IndexError> {
    run_cold_shape_with_prior_tokens(body, record, ceiling, inspect_output, false)
}

fn run_cold_shape_with_prior_tokens(
    body: &str,
    record: bool,
    ceiling: u64,
    inspect_output: bool,
    seed_prior: bool,
) -> Result<ShapeRun, IndexError> {
    let fixture = visitor_fixture()?;
    let sink = Arc::new(ShapeSink::default());
    let mut operation = None;
    let diagnostic = if record {
        let trace = DiagnosticContextV1::new(
            DiagnosticCaptureIdV1([123; 16]),
            Arc::new(ShapeClock::default()),
            sink.clone(),
        );
        let root = trace
            .enter_root(DiagnosticRootKindV1::Operation, DiagnosticLaneV1(0))
            .map_err(|error| IndexError::BackendViolation(error.to_string()))?;
        let child = trace.child_context(&root);
        operation = Some(root);
        Some(child)
    } else {
        None
    };
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
            bytes: 100_000_000,
            mutations: 100_000,
        },
        request,
    );
    // This scope funds the token arena but offers no ordered-output credit.
    // The source can still select a borrowed alias or the SQL fallback.
    let memory = Arc::new(TokenMemory {
        ceiling,
        ..TokenMemory::default()
    });
    work.bind_memory_scope(Some(quanta_index_core::index::MemoryScope::new(Arc::new(
        TokenMemoryScope(memory.clone()),
    ))));
    let scratch = TestScratch::default();
    let custody = TestCustody;
    let spool = TextSpool::new(&scratch, &mut work)?;
    if seed_prior {
        let _seed_count =
            visit_indexed_token_batches("prior_owner_only", &mut work, |rows, budget| {
                spool.token_batch(rows, budget)
            })?;
    }
    let mut edits = InventoryEdits::new(0)?;
    let mut writes = OccurrenceWrites::new();
    writes.pack_owners = true;
    let mut confirmed = ConfirmedOccurrences::new(0)?;
    let mut context = AttemptContext {
        attempt: test_attempt(fixture.ticket.view.identity.workspace),
        objects: &fixture.objects,
        maps: &fixture.maps,
        custody: &custody,
        scratch: &scratch,
        budget: &mut work,
    };
    let prepared = prepare_inventory(
        body,
        &EntryMap::default(),
        0,
        &mut context,
        &spool,
        &mut confirmed,
        &mut edits,
        &mut writes,
    )?;
    let sql_has_terms = !spool.token_term_page(None, context.budget)?.is_empty();
    let pending = writes.pending.len();
    let output_groups = if inspect_output {
        Some(staged_packed_groups(&writes)?)
    } else {
        None
    };
    let result = ShapeRun {
        indexed_tokens: prepared.indexed_tokens,
        term_count: prepared.term_count,
        changed: prepared.changed,
        has_alias: prepared.alias.is_some(),
        pending,
        sql_has_terms,
        work: context.budget.usage(),
        counters: BTreeMap::new(),
        census: None,
        output_groups,
    };
    drop((context, writes, edits, confirmed, spool));
    check_eq!(memory.live.load(Ordering::Relaxed), 0);
    if let Some(operation) = operation {
        operation
            .finish(DiagnosticTerminalV1::Ok)
            .map_err(|error| IndexError::BackendViolation(error.to_string()))?;
    }
    let mut result = result;
    let events = sink
        .0
        .lock()
        .map_err(|error| IndexError::BackendViolation(error.to_string()))?;
    let mut census_spans = std::collections::BTreeSet::new();
    let mut work_bytes = BTreeMap::new();
    for event in events.iter() {
        match event.kind {
            DiagnosticEventKindV1::SpanBegin {
                span_id,
                stage: DiagnosticStageV1::NativeTextInventoryPrepare,
                lane: DiagnosticLaneV1(43),
                ..
            } => {
                check!(census_spans.insert(span_id), "duplicate census span");
            }
            DiagnosticEventKindV1::Counter {
                span_id,
                key: quanta_index_core::DiagnosticCounterV1::WorkBytes,
                value,
            } => {
                check!(
                    work_bytes.insert(span_id, value).is_none(),
                    "duplicate WorkBytes"
                );
            }
            _ => {}
        }
        if let DiagnosticEventKindV1::Counter { key, value, .. } = event.kind {
            let name = format!("{key:?}");
            if matches!(
                name.as_str(),
                "TextCaseOccurrences"
                    | "TextUniqueCanonicalTerms"
                    | "TextOccurrenceKeyBytes"
                    | "TextUniqueKeyBytes"
                    | "TextColdAliasOwners"
                    | "TextColdDirectOwners"
                    | "TextColdSqlOwners"
                    | "TextColdEmptyOwners"
                    | "TextShapeCompleteOwners"
            ) {
                check!(
                    result.counters.insert(name.clone(), value).is_none(),
                    "duplicate shape counter: {name}"
                );
            }
        }
    }
    if record {
        let mut total = 0u64;
        for span in &census_spans {
            total = total
                .checked_add(*work_bytes.get(span).ok_or_else(|| {
                    IndexError::BackendViolation("census span lacks WorkBytes".into())
                })?)
                .ok_or(IndexError::ResourceLimited)?;
        }
        result.census = Some(CensusTrace {
            spans: census_spans.len(),
            work_bytes: total,
        });
    }
    Ok(result)
}

fn assert_shape(
    body: &str,
    indexed_tokens: u64,
    terms: u64,
    expected: [(&str, u64); 6],
) -> Result<(), IndexError> {
    let off = run_cold_shape(body, false)?;
    let recorded = run_cold_shape(body, true)?;
    check_eq!(off.indexed_tokens, indexed_tokens);
    check_eq!(off.term_count, terms);
    check_eq!(off.changed, terms != 0);
    check_eq!(off.has_alias, false);
    check_eq!(off.sql_has_terms, terms != 0);
    check_eq!(off.pending > 0, terms != 0);
    check_eq!(
        (
            off.indexed_tokens,
            off.term_count,
            off.changed,
            off.has_alias,
            off.pending,
            off.sql_has_terms,
        ),
        (
            recorded.indexed_tokens,
            recorded.term_count,
            recorded.changed,
            recorded.has_alias,
            recorded.pending,
            recorded.sql_has_terms,
        )
    );
    check_eq!(off.work, recorded.work);
    check!(off.counters.is_empty());
    for (key, value) in expected {
        check_eq!(recorded.counters.get(key).copied(), Some(value));
    }
    for key in [
        "TextColdAliasOwners",
        "TextColdDirectOwners",
        "TextColdSqlOwners",
        "TextColdEmptyOwners",
    ] {
        let expected = if terms == 0 {
            u64::from(key == "TextColdEmptyOwners")
        } else {
            u64::from(key == "TextColdSqlOwners")
        };
        check_eq!(recorded.counters.get(key).copied().unwrap_or(0), expected);
    }
    Ok(())
}

#[test]
fn cold_sql_shape_emits_fixed_case_key_gold_and_preserves_work() -> Result<(), IndexError> {
    // Sensitive/insensitive case prefixes are included in both byte totals.
    assert_shape(
        "Alpha alpha β",
        3,
        5,
        [
            ("TextCaseOccurrences", 6),
            ("TextUniqueCanonicalTerms", 5),
            ("TextOccurrenceKeyBytes", 30),
            ("TextUniqueKeyBytes", 24),
            ("TextColdSqlOwners", 1),
            ("TextShapeCompleteOwners", 1),
        ],
    )
}

#[test]
fn cold_empty_shape_is_complete_zero_not_missing() -> Result<(), IndexError> {
    assert_shape(
        "",
        0,
        0,
        [
            ("TextCaseOccurrences", 0),
            ("TextUniqueCanonicalTerms", 0),
            ("TextOccurrenceKeyBytes", 0),
            ("TextUniqueKeyBytes", 0),
            ("TextColdEmptyOwners", 1),
            ("TextShapeCompleteOwners", 1),
        ],
    )
}

fn fixed_six_run_groups() -> BTreeMap<Vec<u8>, Vec<u32>> {
    BTreeMap::from([
        (b"\0Alpha".to_vec(), vec![0]),
        (b"\0alpha".to_vec(), vec![1]),
        (b"\x01alpha".to_vec(), vec![0, 1]),
        ("\0β".as_bytes().to_vec(), vec![2]),
        ("\u{1}β".as_bytes().to_vec(), vec![2]),
        (b"\0gamma".to_vec(), vec![3]),
        (b"\x01gamma".to_vec(), vec![3]),
        (b"\0delta".to_vec(), vec![4]),
        (b"\x01delta".to_vec(), vec![4]),
        (b"\0epsilon".to_vec(), vec![5]),
        (b"\x01epsilon".to_vec(), vec![5]),
    ])
}

fn alias_output_denial_ceiling(body: &str) -> Result<u64, IndexError> {
    // Use the same canonical backing and selected output arithmetic as the
    // existing cold-arena admission regression. No threshold search is used.
    let (mut calibration, memory) = funded_token_budget(1_000_000)?;
    let (mut cursor, mut packet) = prepare_indexed_token_input(body, &mut calibration)?;
    let census =
        cold_tokens::CanonicalTokenCensus::count(&mut cursor, body.len(), &mut calibration)?
            .ok_or(IndexError::ResourceLimited)?;
    check!(packet.try_prepare_cold_keys(census.packet_key_bytes(), &calibration)?);
    let mut cold = cold_tokens::ColdTokens::try_new(census, &calibration)?
        .ok_or(IndexError::ResourceLimited)?;
    check_eq!(
        visit_prepared_token_batches(
            &mut cursor,
            &mut packet,
            &mut calibration,
            |rows, budget| { cold.push_batch(rows, budget) }
        )?,
        6
    );
    cold.sort_and_validate(&calibration)?;
    drop((packet, cursor));
    let alias = cold.try_packed_alias(&mut calibration)?.ok_or_else(|| {
        IndexError::BackendViolation("six-run source has no borrowed alias candidate".into())
    })?;
    check_eq!(alias.term_count(), 11);
    let first_key_bytes =
        u64::try_from(alias.first_key()?.len()).map_err(|_error| IndexError::ResourceLimited)?;
    let output_bytes =
        u64::try_from(alias.encoded_len()).map_err(|_error| IndexError::ResourceLimited)?;
    let ceiling = memory
        .live
        .load(Ordering::Relaxed)
        .checked_add(first_key_bytes)
        .and_then(|bytes| bytes.checked_add(output_bytes))
        .and_then(|bytes| bytes.checked_sub(1))
        .ok_or(IndexError::ResourceLimited)?;
    drop(alias);
    drop(cold);
    check_eq!(memory.live.load(Ordering::Relaxed), 0);
    Ok(ceiling)
}

fn assert_six_run_branch(
    ceiling: u64,
    sql_has_terms: bool,
    branch: &str,
) -> Result<(), IndexError> {
    let body = "Alpha alpha β gamma delta epsilon";
    let gold = fixed_six_run_groups();
    check_eq!(gold.len(), 11);
    let off = run_cold_shape_with_ceiling(body, false, ceiling, true)?;
    let recorded = run_cold_shape_with_ceiling(body, true, ceiling, true)?;
    for run in [&off, &recorded] {
        check_eq!(run.indexed_tokens, 6);
        check_eq!(run.term_count, 11);
        check!(run.changed && run.has_alias);
        check_eq!(run.pending, 1);
        check_eq!(run.sql_has_terms, sql_has_terms);
        check_eq!(run.output_groups.as_ref(), Some(&gold));
    }
    check_eq!(off.work, recorded.work);
    check!(off.counters.is_empty());
    check_eq!(recorded.counters.len(), 6);
    for (key, value) in [
        ("TextCaseOccurrences", 12),
        ("TextUniqueCanonicalTerms", 11),
        ("TextOccurrenceKeyBytes", 70),
        ("TextUniqueKeyBytes", 64),
        ("TextShapeCompleteOwners", 1),
        (branch, 1),
    ] {
        check_eq!(recorded.counters.get(key).copied(), Some(value));
    }
    for key in [
        "TextColdAliasOwners",
        "TextColdDirectOwners",
        "TextColdSqlOwners",
        "TextColdEmptyOwners",
    ] {
        check_eq!(
            recorded.counters.get(key).copied().unwrap_or(0),
            u64::from(key == branch)
        );
    }
    Ok(())
}

#[test]
fn cold_borrowed_alias_shape_matches_fixed_group_stream() -> Result<(), IndexError> {
    assert_six_run_branch(16 * 1024 * 1024, false, "TextColdAliasOwners")
}

#[test]
fn cold_sql_inline_alias_reports_sql_branch_and_fixed_group_stream() -> Result<(), IndexError> {
    let body = "Alpha alpha β gamma delta epsilon";
    assert_six_run_branch(
        alias_output_denial_ceiling(body)?,
        true,
        "TextColdSqlOwners",
    )
}

#[test]
fn cold_bound_with_no_indexed_runs_clears_previous_owner_sql() -> Result<(), IndexError> {
    let body = format!("!? {}", "x".repeat(257));
    let off = run_cold_shape_with_prior_tokens(&body, false, 16 * 1024 * 1024, false, true)?;
    let recorded = run_cold_shape_with_prior_tokens(&body, true, 16 * 1024 * 1024, false, true)?;
    for run in [&off, &recorded] {
        check_eq!(run.indexed_tokens, 0);
        check_eq!(run.term_count, 0);
        check!(!run.changed && !run.has_alias && !run.sql_has_terms);
        check_eq!(run.pending, 0);
    }
    check_eq!(off.work, recorded.work);
    check_eq!(
        recorded.census,
        Some(CensusTrace {
            spans: 0,
            work_bytes: 0
        })
    );
    check_eq!(recorded.counters.get("TextColdEmptyOwners"), Some(&1));
    check_eq!(recorded.counters.get("TextShapeCompleteOwners"), Some(&1));
    Ok(())
}

#[test]
fn cold_admitted_arena_skips_census_for_ascii_and_unicode_gold() -> Result<(), IndexError> {
    let mut ascii_gold = fixed_six_run_groups();
    check_eq!(ascii_gold.remove("\0β".as_bytes()), Some(vec![2]));
    check_eq!(ascii_gold.remove("\u{1}β".as_bytes()), Some(vec![2]));
    check!(ascii_gold.insert(b"\0beta".to_vec(), vec![2]).is_none());
    check!(ascii_gold.insert(b"\x01beta".to_vec(), vec![2]).is_none());
    for (body, gold) in [
        ("Alpha alpha beta gamma delta epsilon", ascii_gold),
        ("Alpha alpha β gamma delta epsilon", fixed_six_run_groups()),
    ] {
        let off = run_cold_shape_with_ceiling(body, false, 16 * 1024 * 1024, true)?;
        let recorded = run_cold_shape_with_ceiling(body, true, 16 * 1024 * 1024, true)?;
        for run in [&off, &recorded] {
            check_eq!(run.indexed_tokens, 6);
            check_eq!(run.term_count, 11);
            check!(run.changed && run.has_alias);
            check_eq!(run.pending, 1);
            check!(!run.sql_has_terms);
            check_eq!(run.output_groups.as_ref(), Some(&gold));
        }
        check_eq!(off.work, recorded.work);
        check_eq!(off.census, None);
        check_eq!(
            recorded.census,
            Some(CensusTrace {
                spans: 0,
                work_bytes: 0,
            })
        );
    }
    Ok(())
}

#[test]
fn cold_upper_admission_miss_retains_exact_census_and_sql_fallback() -> Result<(), IndexError> {
    // Four 256-byte indexed runs fit the exact arena with output headroom;
    // the pre-scan Unicode upper bound cannot be admitted at this ceiling.
    let run = "β".repeat(128);
    let body = [run.as_str(); 4].join(" ");
    let off = run_cold_shape_with_ceiling(&body, false, 12_000, false)?;
    let recorded = run_cold_shape_with_ceiling(&body, true, 12_000, false)?;
    for run in [&off, &recorded] {
        check_eq!(run.indexed_tokens, 4);
        check_eq!(run.term_count, 2);
        check!(run.changed);
        check!(!run.has_alias);
        check_eq!(run.pending, 1);
        check!(run.sql_has_terms);
    }
    check_eq!(off.work, recorded.work);
    check_eq!(off.census, None);
    // NFC input is 1027 bytes; eight prefixed case keys contain 2056 bytes.
    check_eq!(
        recorded.census,
        Some(CensusTrace {
            spans: 1,
            work_bytes: 3083,
        })
    );
    check_eq!(recorded.counters.get("TextColdSqlOwners"), Some(&1));
    check_eq!(recorded.counters.get("TextShapeCompleteOwners"), Some(&1));
    Ok(())
}
