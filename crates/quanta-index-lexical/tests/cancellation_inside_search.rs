//! W5 phase 2 — the request budget is observed inside a lexical
//! execution, not only at the dispatcher's checkpoints around it.
//!
//! Every case hands the adapter a budget that is already interrupted, so
//! the only place the interruption can be observed is inside the adapter:
//! in the native collect, in the unindexed scan loop, or in the regex
//! verification. The typed answer names that checkpoint. The same queries
//! under an unbounded budget serve, which pins that the refusal is the
//! budget and not the query.

#![forbid(unsafe_code)]

use std::error::Error;
use std::time::{Duration, Instant};

use quanta_index_contract::lex::LanguageCode;
use quanta_index_contract::{
    BatchIngestMode, ChunkId, ChunkRecord, LQ_VERSION_TAG, LqExpr, LqLeaf, LqOptions, LqQuery,
    LqSpan, LqYesNoOnly, ManifestGeneration, QueryConstraintSetV1, RepoId, RepoRelativePath,
    RevisionId, SearchCorpusIngestBatch, SearchCorpusReplaceScope, SearchScopeKey,
    SearchScopeSurface,
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
    Ok(SearchCorpusReplaceScope {
        scope: SearchScopeKey {
            doc_surface: SearchScopeSurface::File,
            repo_relative_path: RepoRelativePath::new(&path),
        },
        scope_digest: format!("scope:{path}"),
        chunks: vec![ChunkRecord {
            chunk_id: ChunkId::new(format!("chunk-{index}")),
            repo_relative_path: RepoRelativePath::new(&path),
            language,
            start_byte: 0,
            end_byte,
            start_line: 1,
            end_line: 1,
            text: body.into_boxed_str(),
            structural: None,
            parent_chunk_id: None,
            source_repo_id: None,
        }],
        symbols: Vec::new(),
    })
}

fn sealed_batch() -> Result<SearchCorpusIngestBatch, Box<dyn Error>> {
    Ok(SearchCorpusIngestBatch {
        repo_id: repo(),
        revision_id: revision(),
        generation: generation(),
        base_generation: None,
        manifest_digest: "cancel-manifest:1".to_string(),
        batch_digest: "cancel-batch:1".to_string(),
        mode: BatchIngestMode::ReplaceGeneration,
        bundle_payload: None,
        clear_surfaces: Vec::new(),
        replace_scopes: (1..=DOCS).map(scope).collect::<Result<Vec<_>, _>>()?,
        tombstone_scopes: Vec::new(),
        semantic_replace_scopes: Vec::new(),
        semantic_tombstone_scopes: Vec::new(),
        seal: true,
    })
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
    adapter.build_batch(&sealed_batch()?)?;
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

/// The native collect observes the budget: a cancelled request and a
/// passed deadline are both answered from inside the collect.
#[test]
fn a_cancelled_or_expired_budget_is_observed_inside_the_native_collect() -> TestResult {
    let (_dir, adapter) = seeded()?;
    let searcher = adapter.open(&repo(), &revision(), generation())?;
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
        "lexical:collect",
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
        "lexical:collect",
    )
}

/// Regex verification walks every prefiltered candidate; the budget is
/// observed between candidates, before any collect runs.
///
/// The interrupted query runs first: a verified set is cached by pattern
/// (QI-BB-024), and a cached set has no verification loop to interrupt,
/// so the same query served once would be observed at the collect
/// instead. That the control afterwards serves from the cache is the
/// second half of the proof.
#[test]
fn a_cancelled_budget_is_observed_inside_regex_verification() -> TestResult {
    let (_dir, adapter) = seeded()?;
    let searcher = adapter.open(&repo(), &revision(), generation())?;
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
        "lexical:regex-verify",
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
    // Served once, the verified set is cached and a cancelled request is
    // observed at the collect, not in a verification that no longer runs.
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
        "lexical:collect",
    )
}

/// An `index:no` scan walks every stored document; the budget is observed
/// in that loop.
#[test]
fn a_cancelled_budget_is_observed_inside_the_unindexed_scan() -> TestResult {
    let (_dir, adapter) = seeded()?;
    let searcher = adapter.open(&repo(), &revision(), generation())?;
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
        "lexical:scan",
    )
}
