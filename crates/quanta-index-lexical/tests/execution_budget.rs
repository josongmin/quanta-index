//! QI-BB-005 — one lexical execution may not materialize more than the
//! examined-candidate budget allows.
//!
//! A page query never needed the whole match set, but projections, bounded
//! counts and `index:no` scans did, and they collected `num_docs` for it. The
//! budget makes that collect observable: an exact-set execution that would
//! overrun it is refused with `LEXICAL_EXAMINED_BUDGET_EXCEEDED`, while page
//! queries over the same corpus — and exact totals that need no
//! materialization — keep serving.
//!
//! The budget is injected small here so the corpus stays small; the
//! production default lives in core's `LexicalExecutionBudgetV1::DEFAULT`.

#![forbid(unsafe_code)]

#[path = "support/source_fixture.rs"]
mod source_fixture;

use std::error::Error;

use quanta_index_contract::lex::LanguageCode;
use quanta_index_contract::{
    ChunkId, ChunkRecord, LQ_VERSION_TAG, LqCountBound, LqExpr, LqFileScope, LqFilter, LqLeaf,
    LqOptions, LqPredicateArg, LqQuery, LqSelect, LqSpan, LqType, LqYesNoOnly, ManifestGeneration,
    QueryConstraintSetV1, RepoId, RepoRelativePath, RevisionId, SearchCorpusIngestBatch,
    SearchCorpusReplaceScope, SearchPlaneErrorCodeV2,
};
use quanta_index_core::{
    CoreError, LEXICAL_EXAMINED_BUDGET_EXCEEDED_CODE, LexicalExecutionBudgetV1,
    LexicalIndexOpenPort, LexicalPageSpec, LexicalWriterPolicy, RegexMatchCachePolicy,
    RequestBudgetV1, SearchCorpusBatchBuildPort,
};
use quanta_index_lexical::LexicalAdapter;
use quanta_index_lexical::regex::RegexPolicy;

type TestResult = Result<(), Box<dyn Error>>;

/// Small enough that a handful of documents overrun it.
const BUDGET: usize = 4;
/// One more than the budget: exact-set executions over all of them must refuse.
const DOCS: u32 = 5;

#[expect(
    clippy::expect_used,
    reason = "static fixture IDs provably satisfy the canonical ID policy"
)]
fn repo() -> RepoId {
    RepoId::new("budget-repo").expect("static fixture ID satisfies canonical policy")
}

#[expect(
    clippy::expect_used,
    reason = "static fixture IDs provably satisfy the canonical ID policy"
)]
fn revision() -> RevisionId {
    RevisionId::new("budget-rev").expect("static fixture ID satisfies canonical policy")
}

fn generation() -> ManifestGeneration {
    ManifestGeneration::new(1)
}

fn scope(index: u32) -> Result<SearchCorpusReplaceScope, Box<dyn Error>> {
    let path = format!("src/file_{index}.rs");
    let body = format!("fn item_{index}() {{ budget_needle }}");
    let language = LanguageCode::new("rust")
        .map_err(|err| -> Box<dyn Error> { format!("language code: {err}").into() })?;
    let end_byte = u32::try_from(body.len())?;
    Ok(source_fixture::complete_file(
        source_fixture::file_key(&repo(), &path),
        &revision(),
        language.clone(),
        body.as_bytes(),
        vec![ChunkRecord {
            chunk_id: ChunkId::new(format!("chunk-{index}")),
            repo_relative_path: RepoRelativePath::new(&path),
            language,
            start_byte: 0,
            end_byte,
            start_line: 1,
            end_line: 1,
            text: body.clone().into_boxed_str(),
            structural: None,
            parent_chunk_id: None,
            source_repo_id: None,
        }],
        Vec::new(),
    )?)
}

fn sealed_batch() -> Result<SearchCorpusIngestBatch, Box<dyn Error>> {
    Ok(source_fixture::sealed_batch(
        &repo(),
        &revision(),
        generation(),
        (1..=DOCS).map(scope).collect::<Result<Vec<_>, _>>()?,
    )?)
}

fn query(expr: LqExpr, filters: Vec<LqFilter>) -> LqQuery {
    LqQuery {
        lq_version: LQ_VERSION_TAG,
        expr,
        filters,
        options: LqOptions::defaults(),
        directives: Vec::new(),
        source_span: LqSpan::eof(0),
    }
}

fn needle() -> LqExpr {
    LqExpr::Leaf(LqLeaf::Keyword("budget_needle".to_string()))
}

fn adapter_with_budget(
    root: std::path::PathBuf,
    budget: usize,
) -> Result<LexicalAdapter, Box<dyn Error>> {
    Ok(LexicalAdapter::with_state_root_and_policies(
        root,
        RegexPolicy::defaults(),
        LexicalExecutionBudgetV1::new(budget)?,
        RegexMatchCachePolicy::DEFAULT,
        LexicalWriterPolicy::DEFAULT,
    ))
}

fn is_budget_refusal(err: &CoreError) -> bool {
    matches!(err, CoreError::Typed { code, .. } if *code == LEXICAL_EXAMINED_BUDGET_EXCEEDED_CODE)
}

fn seeded(budget: usize) -> Result<(tempfile::TempDir, LexicalAdapter), Box<dyn Error>> {
    let dir = tempfile::tempdir()?;
    let adapter = adapter_with_budget(dir.path().to_path_buf(), budget)?;
    adapter.build_batch(&sealed_batch()?)?;
    Ok((dir, adapter))
}

#[test]
#[expect(
    clippy::panic_in_result_fn,
    reason = "fixed oracle assertions in a fallible fixture"
)]
fn predicate_scope_refuses_materialization_past_the_examined_budget() -> TestResult {
    let scoped = query(
        LqExpr::All(vec![
            LqExpr::Leaf(LqLeaf::Predicate {
                name: "repo.has.file".to_string(),
                args: vec![LqPredicateArg::Filter {
                    name: "path".to_string(),
                    value: "src/file_.*".to_string(),
                }],
            }),
            needle(),
        ]),
        Vec::new(),
    );
    let (_dir, adapter) = seeded(BUDGET)?;
    let searcher = adapter.open(&repo(), &revision(), generation())?;
    let refused = searcher.search_constrained(
        &scoped,
        &QueryConstraintSetV1::unconstrained(),
        &LexicalPageSpec::first(2),
        &RequestBudgetV1::unbounded(),
    );
    assert!(
        matches!(&refused, Err(error) if is_budget_refusal(error)),
        "predicate scope must refuse a partial gate: {refused:?}"
    );

    let (_dir, adapter) = seeded(usize::try_from(DOCS)?)?;
    let searcher = adapter.open(&repo(), &revision(), generation())?;
    let page = searcher.search_constrained(
        &scoped,
        &QueryConstraintSetV1::unconstrained(),
        &LexicalPageSpec::first(2),
        &RequestBudgetV1::unbounded(),
    )?;
    assert_eq!(page.candidates.len(), 2);
    Ok(())
}

/// A page and an exact total both stay within budget; a projection over the
/// same matches, which must see them all, is refused under the typed code.
///
/// A bounded count (`count:N`) is a page: the ranked collector cuts it in
/// exact order and counts every match in the same pass, so it serves like
/// `count:all` and never collects the match set (QI-BB-005).
#[test]
fn exact_set_executions_over_the_budget_are_refused_but_pages_serve() -> TestResult {
    let (_dir, adapter) = seeded(BUDGET)?;
    let searcher = adapter.open(&repo(), &revision(), generation())?;
    let unconstrained = QueryConstraintSetV1::unconstrained();

    // A page over five matches: two rows, no materialization beyond the probe.
    let page = searcher.search_constrained(
        &query(needle(), Vec::new()),
        &unconstrained,
        &LexicalPageSpec::first(2),
        &RequestBudgetV1::unbounded(),
    )?;
    if page.candidates.len() != 2 || page.exact_total.is_some() {
        return Err(format!(
            "page drifted: rows={} exact_total={:?}",
            page.candidates.len(),
            page.exact_total
        )
        .into());
    }

    // `count:all` needs the count collector, not the documents: still serves.
    let mut counted = query(needle(), Vec::new());
    counted.options.count = Some(LqCountBound::All);
    let counted_page = searcher.search_constrained(
        &counted,
        &unconstrained,
        &LexicalPageSpec::first(2),
        &RequestBudgetV1::unbounded(),
    )?;
    if counted_page.candidates.len() != 2 || counted_page.exact_total != Some(u64::from(DOCS)) {
        return Err(format!(
            "count:all drifted: rows={} exact_total={:?}",
            counted_page.candidates.len(),
            counted_page.exact_total
        )
        .into());
    }

    // A path projection must collapse the whole match set: five > budget.
    let projected = query(
        needle(),
        vec![LqFilter::Select {
            dim: LqSelect::Path,
        }],
    );
    match searcher.search_constrained(
        &projected,
        &unconstrained,
        &LexicalPageSpec::first(2),
        &RequestBudgetV1::unbounded(),
    ) {
        Err(err) if is_budget_refusal(&err) => {}
        Err(other) => {
            return Err(format!("projection refused under the wrong error: {other}").into());
        }
        Ok(page) => {
            return Err(format!(
                "projection over {DOCS} matches served {} rows under a budget of {BUDGET}",
                page.candidates.len()
            )
            .into());
        }
    }

    // A bounded count is a page with a count: it serves.
    let mut bounded = query(needle(), Vec::new());
    bounded.options.count = Some(LqCountBound::Bounded(2));
    let bounded_page = searcher.search_constrained(
        &bounded,
        &unconstrained,
        &LexicalPageSpec::first(3),
        &RequestBudgetV1::unbounded(),
    )?;
    if bounded_page.candidates.len() != 2 || bounded_page.exact_total != Some(u64::from(DOCS)) {
        return Err(format!(
            "count:2 drifted: rows={} exact_total={:?}",
            bounded_page.candidates.len(),
            bounded_page.exact_total
        )
        .into());
    }
    Ok(())
}

/// The same exact-set executions serve once the budget admits the match set,
/// which pins that the refusal is the budget and not the query shape.
#[test]
fn exact_set_executions_within_the_budget_serve_with_exact_totals() -> TestResult {
    let (_dir, adapter) = seeded(usize::try_from(DOCS)?)?;
    let searcher = adapter.open(&repo(), &revision(), generation())?;
    let unconstrained = QueryConstraintSetV1::unconstrained();

    let projected = query(
        needle(),
        vec![LqFilter::Select {
            dim: LqSelect::Path,
        }],
    );
    let page = searcher.search_constrained(
        &projected,
        &unconstrained,
        &LexicalPageSpec::first(2),
        &RequestBudgetV1::unbounded(),
    )?;
    if page.candidates.len() != 2 || page.exact_total != Some(u64::from(DOCS)) {
        return Err(format!(
            "projection within budget drifted: rows={} exact_total={:?}",
            page.candidates.len(),
            page.exact_total
        )
        .into());
    }

    let mut bounded = query(needle(), Vec::new());
    bounded.options.count = Some(LqCountBound::Bounded(3));
    let page = searcher.search_constrained(
        &bounded,
        &unconstrained,
        &LexicalPageSpec::first(10),
        &RequestBudgetV1::unbounded(),
    )?;
    if page.candidates.len() != 3 || page.exact_total != Some(u64::from(DOCS)) {
        return Err(format!(
            "count:3 within budget drifted: rows={} exact_total={:?}",
            page.candidates.len(),
            page.exact_total
        )
        .into());
    }
    Ok(())
}

/// An explicit `index:no` scan examines the whole corpus, so the corpus
/// itself must fit the budget before the scan starts.
#[test]
fn unindexed_scans_over_the_budget_are_refused_before_scanning() -> TestResult {
    let (_dir, adapter) = seeded(BUDGET)?;
    let searcher = adapter.open(&repo(), &revision(), generation())?;
    let mut scan = query(needle(), Vec::new());
    scan.options.index_mode = Some(LqYesNoOnly::No);
    match searcher.search_constrained(
        &scan,
        &QueryConstraintSetV1::unconstrained(),
        &LexicalPageSpec::first(2),
        &RequestBudgetV1::unbounded(),
    ) {
        Err(err) if is_budget_refusal(&err) => {}
        Err(other) => return Err(format!("scan refused under the wrong error: {other}").into()),
        Ok(page) => {
            return Err(format!(
                "index:no over {DOCS} docs served {} rows under a budget of {BUDGET}",
                page.candidates.len()
            )
            .into());
        }
    }

    // The symbol form of the same scan shares the gate.
    let mut symbol_scan = query(
        needle(),
        vec![LqFilter::Type {
            kind: LqType::Symbol,
        }],
    );
    symbol_scan.options.index_mode = Some(LqYesNoOnly::No);
    match searcher.search_symbols_constrained(
        &symbol_scan,
        &QueryConstraintSetV1::unconstrained(),
        &LexicalPageSpec::first(2),
        &RequestBudgetV1::unbounded(),
    ) {
        Err(err) if is_budget_refusal(&err) => Ok(()),
        Err(other) => Err(format!("symbol scan refused under the wrong error: {other}").into()),
        Ok(rows) => Err(format!(
            "symbol index:no over {DOCS} docs served {} rows under a budget of {BUDGET}",
            rows.candidates.len()
        )
        .into()),
    }
}

#[test]
#[expect(
    clippy::panic_in_result_fn,
    reason = "fixed oracle assertions in a fallible fixture"
)]
fn unindexed_scan_obeys_native_collection_byte_limit() -> TestResult {
    let dir = tempfile::tempdir()?;
    let adapter = LexicalAdapter::with_state_root_and_policies(
        dir.path().to_path_buf(),
        RegexPolicy::defaults(),
        LexicalExecutionBudgetV1::new_with_collection_bytes(usize::try_from(DOCS)?, 1)?,
        RegexMatchCachePolicy::DEFAULT,
        LexicalWriterPolicy::DEFAULT,
    );
    adapter.build_batch(&sealed_batch()?)?;
    let searcher = adapter.open(&repo(), &revision(), generation())?;
    let mut scan = query(needle(), Vec::new());
    scan.options.index_mode = Some(LqYesNoOnly::No);
    let refused = searcher.search_constrained(
        &scan,
        &QueryConstraintSetV1::unconstrained(),
        &LexicalPageSpec::first(2),
        &RequestBudgetV1::unbounded(),
    );
    assert!(
        matches!(
            &refused,
            Err(CoreError::Typed {
                code: SearchPlaneErrorCodeV2::LexicalCollectionBudgetExceeded,
                ..
            })
        ),
        "index:no scan bypassed its retained-byte limit: {refused:?}"
    );
    Ok(())
}

#[test]
#[expect(
    clippy::panic_in_result_fn,
    reason = "fixed corpus checks the manual repo-gate cache boundary through the public search route"
)]
fn unindexed_repo_gates_refuse_excess_distinct_materializations() -> TestResult {
    let (_dir, adapter) = seeded(usize::try_from(DOCS)?)?;
    let searcher = adapter.open(&repo(), &revision(), generation())?;
    let absent_gates = |count| {
        LqExpr::Any(
            (0..count)
                .map(|index| {
                    LqExpr::Leaf(LqLeaf::Predicate {
                        name: "repo.has.content".to_string(),
                        args: vec![LqPredicateArg::Keyword(format!("absent_gate_{index}"))],
                    })
                })
                .collect(),
        )
    };
    let mut allowed = query(absent_gates(64), Vec::new());
    allowed.options.index_mode = Some(LqYesNoOnly::No);
    let page = searcher.search_constrained(
        &allowed,
        &QueryConstraintSetV1::unconstrained(),
        &LexicalPageSpec::first(2),
        &RequestBudgetV1::unbounded(),
    )?;
    assert!(page.candidates.is_empty());

    let mut excessive = query(absent_gates(65), Vec::new());
    excessive.options.index_mode = Some(LqYesNoOnly::No);
    let refused = searcher.search_constrained(
        &excessive,
        &QueryConstraintSetV1::unconstrained(),
        &LexicalPageSpec::first(2),
        &RequestBudgetV1::unbounded(),
    );
    assert!(
        matches!(
            refused,
            Err(CoreError::Typed {
                code: SearchPlaneErrorCodeV2::LexicalCollectionBudgetExceeded,
                ..
            })
        ),
        "the 65th distinct repo gate must refuse the entire request"
    );
    Ok(())
}

#[test]
#[expect(
    clippy::panic_in_result_fn,
    reason = "fixed oracle assertions in a fallible fixture"
)]
fn dense_admission_refuses_candidate_set_over_examined_limit() -> TestResult {
    let (_dir, adapter) = seeded(BUDGET)?;
    let searcher = adapter.open(&repo(), &revision(), generation())?;
    let candidate_ids = (1..=DOCS)
        .map(|id| format!("candidate-{id}"))
        .collect::<std::collections::BTreeSet<_>>();
    let refused = searcher.admitted_candidates(
        &query(LqExpr::Empty, Vec::new()),
        &QueryConstraintSetV1::unconstrained(),
        &candidate_ids,
        &RequestBudgetV1::unbounded(),
    );
    assert!(
        matches!(&refused, Err(error) if is_budget_refusal(error)),
        "dense admission accepted more candidates than the examined limit: {refused:?}"
    );
    Ok(())
}

#[test]
fn dense_manual_regex_admission_matches_the_independent_path_oracle() -> TestResult {
    let (_dir, adapter) = seeded(usize::try_from(DOCS)?)?;
    let searcher = adapter.open(&repo(), &revision(), generation())?;
    let candidate_ids = (1..=DOCS)
        .map(|id| format!("chunk-{id}"))
        .collect::<std::collections::BTreeSet<_>>();
    let expected = ["chunk-1".to_string(), "chunk-2".to_string()]
        .into_iter()
        .collect::<std::collections::BTreeSet<_>>();
    let mut manual = query(
        LqExpr::Leaf(LqLeaf::Regex(r"item_[12]\(\)".into())),
        vec![LqFilter::File {
            pattern: r"^src/file_[12]\.rs$".into(),
            scope: LqFileScope::PathOnly,
        }],
    );
    manual.options.index_mode = Some(LqYesNoOnly::No);
    let admitted = searcher.admitted_candidates(
        &manual,
        &QueryConstraintSetV1::unconstrained(),
        &candidate_ids,
        &RequestBudgetV1::unbounded(),
    )?;
    if expected.len() != 2 || admitted != expected {
        return Err(format!(
            "manual dense regex admission drifted: expected={expected:?} actual={admitted:?}"
        )
        .into());
    }
    Ok(())
}

/// A zero budget is a configuration defect, not a disabled cap.
#[test]
fn zero_budget_is_refused_at_construction() {
    assert!(LexicalExecutionBudgetV1::new(0).is_err());
    assert!(LexicalExecutionBudgetV1::new(1).is_ok());
}
