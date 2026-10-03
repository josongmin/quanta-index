//! Pre-interrupted requests stop at primitive admission before native work.
//!
//! Each route still returns the canonical cancellation/deadline code and names
//! the checkpoint that observed it. Under an unbounded budget the same query
//! serves its independently fixed row count. These entry tests do not claim a
//! mid-loop interruption; direct BudgetProbe/collector tests cover that layer.

#![forbid(unsafe_code)]

#[path = "support/source_fixture.rs"]
mod source_fixture;

use std::error::Error;
use std::time::{Duration, Instant};

use quanta_index_contract::lex::LanguageCode;
use quanta_index_contract::{
    ChunkId, ChunkRecord, LQ_VERSION_TAG, LqExpr, LqLeaf, LqOptions, LqQuery, LqSpan, LqYesNoOnly,
    ManifestGeneration, QueryConstraintSetV1, RepoId, RepoRelativePath, RevisionId,
    SearchCorpusIngestBatch, SearchCorpusReplaceScope,
};
use quanta_index_core::{
    CoreError, LexicalExecutionBudgetV1, LexicalIndexOpenPort, LexicalPageSpec,
    LexicalWriterPolicy, REQUEST_CANCELLED_CODE, REQUEST_DEADLINE_EXCEEDED_CODE,
    RegexMatchCachePolicy, RequestBudgetV1, SearchCorpusBatchBuildPort,
};
use quanta_index_lexical::LexicalAdapter;
use quanta_index_lexical::regex::RegexPolicy;

type TestResult = Result<(), Box<dyn Error>>;

/// Enough documents that every path has work to interrupt, and more than
/// one probe interval so an interruption is observed mid-walk.
const DOCS: u32 = 3_000;

#[expect(
    clippy::expect_used,
    reason = "static fixture IDs provably satisfy the canonical ID policy"
)]
fn repo() -> RepoId {
    RepoId::new("cancel-repo").expect("static fixture ID satisfies canonical policy")
}

#[expect(
    clippy::expect_used,
    reason = "static fixture IDs provably satisfy the canonical ID policy"
)]
fn revision() -> RevisionId {
    RevisionId::new("cancel-rev").expect("static fixture ID satisfies canonical policy")
}

fn generation() -> ManifestGeneration {
    ManifestGeneration::new(1)
}

fn scope(index: u32) -> Result<SearchCorpusReplaceScope, Box<dyn Error>> {
    let path = format!("src/file_{index}.rs");
    let body = format!("fn item_{index}() {{ cancel_needle token_{index} }}");
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

fn query(expr: LqExpr, options: LqOptions) -> LqQuery {
    LqQuery {
        lq_version: LQ_VERSION_TAG,
        expr,
        filters: Vec::new(),
        options,
        directives: Vec::new(),
        source_span: LqSpan::eof(0),
    }
}

fn keyword_query() -> LqQuery {
    query(
        LqExpr::Leaf(LqLeaf::Keyword("cancel_needle".to_string())),
        LqOptions::defaults(),
    )
}

/// A regex whose literal prefilter admits every document, so verification
/// walks the whole corpus.
fn regex_query() -> LqQuery {
    query(
        LqExpr::Leaf(LqLeaf::Regex("token_[0-9]+".to_string())),
        LqOptions::defaults(),
    )
}

fn unindexed_query() -> LqQuery {
    let mut options = LqOptions::defaults();
    options.index_mode = Some(LqYesNoOnly::No);
    query(
        LqExpr::Leaf(LqLeaf::Keyword("cancel_needle".to_string())),
        options,
    )
}

fn seeded() -> Result<(tempfile::TempDir, LexicalAdapter), Box<dyn Error>> {
    let dir = tempfile::tempdir()?;
    let adapter = LexicalAdapter::with_state_root_and_policies(
        dir.path().to_path_buf(),
        RegexPolicy::defaults(),
        LexicalExecutionBudgetV1::new(usize::try_from(DOCS)?)?,
        RegexMatchCachePolicy::DEFAULT,
        LexicalWriterPolicy::DEFAULT,
    );
    let _stages = adapter.build_batch(&sealed_batch()?)?;
    Ok((dir, adapter))
}

fn cancelled_budget() -> RequestBudgetV1 {
    let budget = RequestBudgetV1::unbounded();
    budget.cancel_handle().cancel();
    budget
}

fn passed_deadline_budget() -> Result<RequestBudgetV1, Box<dyn Error>> {
    let deadline = Instant::now()
        .checked_sub(Duration::from_secs(1))
        .ok_or("clock underflow")?;
    Ok(RequestBudgetV1::until(deadline))
}

fn expect_interrupted(
    outcome: Result<usize, CoreError>,
    code: quanta_index_contract::SearchPlaneErrorCodeV2,
    checkpoint: &str,
) -> TestResult {
    match outcome {
        Err(CoreError::Typed {
            code: observed,
            message,
        }) if observed == code && message.contains(&format!("checkpoint `{checkpoint}`")) => Ok(()),
        Err(other) => {
            Err(format!("expected {code} observed at `{checkpoint}`, got {other}").into())
        }
        Ok(rows) => {
            Err(format!("expected {code} observed at `{checkpoint}`, got {rows} rows").into())
        }
    }
}

/// Native search rejects an already interrupted budget before planning work.
#[test]
fn a_cancelled_or_expired_budget_is_refused_before_native_collection() -> TestResult {
    let (_dir, adapter) = seeded()?;
    let searcher = adapter.open(
        &repo(),
        &revision(),
        generation(),
        &quanta_index_core::RequestBudgetV1::unbounded(),
    )?;
    let unconstrained = QueryConstraintSetV1::unconstrained();

    let served = searcher.search_constrained(
        &keyword_query(),
        &unconstrained,
        &LexicalPageSpec::first(10),
        &RequestBudgetV1::unbounded(),
    )?;
    if served.candidates.len() != 10 {
        return Err(format!(
            "the control page serves 10 rows, got {}",
            served.candidates.len()
        )
        .into());
    }
    expect_interrupted(
        searcher
            .search_constrained(
                &keyword_query(),
                &unconstrained,
                &LexicalPageSpec::first(10),
                &cancelled_budget(),
            )
            .map(|page| page.candidates.len()),
        REQUEST_CANCELLED_CODE,
        "lexical primitive admission",
    )?;
    expect_interrupted(
        searcher
            .search_constrained(
                &keyword_query(),
                &unconstrained,
                &LexicalPageSpec::first(10),
                &passed_deadline_budget()?,
            )
            .map(|page| page.candidates.len()),
        REQUEST_DEADLINE_EXCEEDED_CODE,
        "lexical primitive admission",
    )
}

#[test]
fn cold_open_rejects_cancelled_and_expired_budgets() -> TestResult {
    let (_dir, adapter) = seeded()?;
    for (budget, expected) in [
        (cancelled_budget(), REQUEST_CANCELLED_CODE),
        (passed_deadline_budget()?, REQUEST_DEADLINE_EXCEEDED_CODE),
    ] {
        match adapter.open(&repo(), &revision(), generation(), &budget) {
            Err(CoreError::Typed { code, .. }) if code == expected => {}
            Err(other) => return Err(format!("cold open returned {other}").into()),
            Ok(_) => return Err("cold open succeeded under interrupted budget".into()),
        }
    }
    let _live = adapter.open(
        &repo(),
        &revision(),
        generation(),
        &RequestBudgetV1::unbounded(),
    )?;
    Ok(())
}

/// Both cold and warm regex queries reject an already cancelled request at
/// admission. The successful middle query remains the execution control.
#[test]
fn a_cancelled_budget_is_refused_before_cold_or_warm_regex_execution() -> TestResult {
    let (_dir, adapter) = seeded()?;
    let searcher = adapter.open(
        &repo(),
        &revision(),
        generation(),
        &quanta_index_core::RequestBudgetV1::unbounded(),
    )?;
    let unconstrained = QueryConstraintSetV1::unconstrained();

    expect_interrupted(
        searcher
            .search_constrained(
                &regex_query(),
                &unconstrained,
                &LexicalPageSpec::first(10),
                &cancelled_budget(),
            )
            .map(|page| page.candidates.len()),
        REQUEST_CANCELLED_CODE,
        "lexical primitive admission",
    )?;
    let served = searcher.search_constrained(
        &regex_query(),
        &unconstrained,
        &LexicalPageSpec::first(10),
        &RequestBudgetV1::unbounded(),
    )?;
    if served.candidates.len() != 10 {
        return Err(format!(
            "the control regex page serves 10 rows, got {}",
            served.candidates.len()
        )
        .into());
    }
    // A warm match cache does not bypass request admission.
    expect_interrupted(
        searcher
            .search_constrained(
                &regex_query(),
                &unconstrained,
                &LexicalPageSpec::first(10),
                &cancelled_budget(),
            )
            .map(|page| page.candidates.len()),
        REQUEST_CANCELLED_CODE,
        "lexical primitive admission",
    )
}

/// The manual route observes a pre-cancelled budget before scanning documents.
#[test]
fn a_cancelled_budget_is_refused_before_the_unindexed_scan() -> TestResult {
    let (_dir, adapter) = seeded()?;
    let searcher = adapter.open(
        &repo(),
        &revision(),
        generation(),
        &quanta_index_core::RequestBudgetV1::unbounded(),
    )?;
    let unconstrained = QueryConstraintSetV1::unconstrained();

    let served = searcher.search_constrained(
        &unindexed_query(),
        &unconstrained,
        &LexicalPageSpec::first(10),
        &RequestBudgetV1::unbounded(),
    )?;
    if served.candidates.len() != 10 || served.exact_total != Some(u64::from(DOCS)) {
        return Err(format!(
            "the control scan serves 10 rows of {DOCS}, got {} / {:?}",
            served.candidates.len(),
            served.exact_total
        )
        .into());
    }
    expect_interrupted(
        searcher
            .search_constrained(
                &unindexed_query(),
                &unconstrained,
                &LexicalPageSpec::first(10),
                &cancelled_budget(),
            )
            .map(|page| page.candidates.len()),
        REQUEST_CANCELLED_CODE,
        "lexical primitive admission",
    )
}
