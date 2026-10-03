//! Regex leaves must not lose documents to their own prefilter.
//!
//! The trigram prefilter is fed the literal set a regex extractor produced.
//! That set is an **alternation** — a match needs one member, not all of them.
//! Two shapes make the distinction load-bearing:
//!
//! * an explicit alternation, `/(alpha|beta)/`, where a document holding only
//!   one branch must still be returned; and
//! * any case-insensitive pattern containing a letter with a non-ASCII
//!   case-fold partner. `(?i)fresh` extracts both `fresh` and `freſh`
//!   (U+017F LATIN SMALL LETTER LONG S), and no ASCII document holds the
//!   second.
//!
//! Intersecting the set instead of unioning it returned zero hits for both,
//! silently, while the keyword route answered the same text correctly — two
//! views of one generation disagreeing. These tests assert only on returned
//! candidate ids, so they stay valid regardless of how the prefilter is
//! implemented.

#![forbid(unsafe_code)]

#[path = "support/source_fixture.rs"]
mod source_fixture;

use std::error::Error;

use quanta_index_contract::lex::LanguageCode;
use quanta_index_contract::{
    ChunkId, ChunkRecord, LQ_VERSION_TAG, LqExpr, LqLeaf, LqOptions, LqQuery, LqSpan,
    ManifestGeneration, RepoId, RepoRelativePath, RevisionId, SearchCorpusReplaceScope,
};
use quanta_index_core::{
    LexicalIndexOpenPort, LexicalSearcher, RequestBudgetV1, SearchCorpusBatchBuildPort,
};
use quanta_index_lexical::LexicalAdapter;

type TestResult = Result<(), Box<dyn Error>>;

#[expect(
    clippy::expect_used,
    reason = "static fixture IDs provably satisfy the canonical ID policy"
)]
fn repo() -> RepoId {
    RepoId::new("regex-alternation-repo").expect("static fixture ID satisfies canonical policy")
}

#[expect(
    clippy::expect_used,
    reason = "static fixture IDs provably satisfy the canonical ID policy"
)]
fn revision() -> RevisionId {
    RevisionId::new("regex-alternation-rev").expect("static fixture ID satisfies canonical policy")
}

fn chunk(path: &str, chunk_id: &str, body: &str) -> Result<ChunkRecord, Box<dyn Error>> {
    Ok(ChunkRecord {
        chunk_id: ChunkId::new(chunk_id),
        repo_relative_path: RepoRelativePath::new(path),
        language: LanguageCode::new("rust")
            .map_err(|err| -> Box<dyn Error> { format!("language code: {err}").into() })?,
        start_byte: 0,
        end_byte: u32::try_from(body.len())
            .map_err(|err| -> Box<dyn Error> { format!("chunk too large: {err}").into() })?,
        start_line: 1,
        end_line: 1,
        text: body.to_string().into_boxed_str(),
        structural: None,
        parent_chunk_id: None,
        source_repo_id: None,
    })
}

fn scope(
    path: &str,
    chunk_id: &str,
    body: &str,
) -> Result<SearchCorpusReplaceScope, Box<dyn Error>> {
    Ok(source_fixture::complete_file(
        source_fixture::file_key(&repo(), path),
        &revision(),
        LanguageCode::new("rust").map_err(str::to_string)?,
        body.as_bytes(),
        vec![chunk(path, chunk_id, body)?],
        Vec::new(),
    )?)
}

/// One sealed generation holding `(path, chunk_id, body)` rows.
fn build(adapter: &LexicalAdapter, rows: &[(&str, &str, &str)]) -> TestResult {
    let generation = ManifestGeneration::new(1);
    let mut replace_scopes = Vec::with_capacity(rows.len());
    for (path, chunk_id, body) in rows {
        replace_scopes.push(scope(path, chunk_id, body)?);
    }
    let _stages = adapter.build_batch(&source_fixture::sealed_batch(
        &repo(),
        &revision(),
        generation,
        replace_scopes,
    )?)?;
    Ok(())
}

fn query(leaf: LqLeaf) -> LqQuery {
    LqQuery {
        lq_version: LQ_VERSION_TAG,
        expr: LqExpr::Leaf(leaf),
        filters: Vec::new(),
        options: LqOptions::defaults(),
        directives: Vec::new(),
        source_span: LqSpan::eof(0),
    }
}

fn hit_ids(searcher: &dyn LexicalSearcher, leaf: LqLeaf) -> Result<Vec<String>, Box<dyn Error>> {
    let mut ids: Vec<String> = searcher
        .search(&query(leaf), 32, &RequestBudgetV1::unbounded())?
        .iter()
        .map(|candidate| candidate.candidate_id.clone())
        .collect();
    ids.sort();
    Ok(ids)
}

fn assert_ids(
    searcher: &dyn LexicalSearcher,
    leaf: LqLeaf,
    expected: &[&str],
    label: &str,
) -> TestResult {
    let observed = hit_ids(searcher, leaf)?;
    let expected_owned: Vec<String> = expected.iter().map(|id| (*id).to_string()).collect();
    if observed != expected_owned {
        return Err(format!("{label}: expected {expected_owned:?}, got {observed:?}").into());
    }
    Ok(())
}

fn open(adapter: &LexicalAdapter) -> Result<Box<dyn LexicalSearcher>, Box<dyn Error>> {
    Ok(adapter.open(
        &repo(),
        &revision(),
        ManifestGeneration::new(1),
        &quanta_index_core::RequestBudgetV1::unbounded(),
    )?)
}

/// A literal regex must find what the keyword route finds.
///
/// `freshsentinel` contains `s`, whose Unicode case-fold partner `ſ` appears in
/// the extracted alternation once the query is normalized to `(?i)`.
#[test]
fn case_folded_literal_regex_matches_the_same_text_as_the_keyword_route() -> TestResult {
    let dir = tempfile::tempdir()?;
    let adapter = LexicalAdapter::with_state_root(dir.path().to_path_buf());
    build(
        &adapter,
        &[
            ("src/a.rs", "c-a", "gamma_replacement freshsentinel"),
            ("src/b.rs", "c-b", "unrelated content without the token"),
        ],
    )?;
    let searcher = open(&adapter)?;

    // Keyword route: the oracle for what the generation holds.
    assert_ids(
        searcher.as_ref(),
        LqLeaf::Keyword("freshsentinel".to_string()),
        &["c-a"],
        "keyword freshsentinel",
    )?;
    // Regex route must agree.
    assert_ids(
        searcher.as_ref(),
        LqLeaf::Regex("freshsentinel".to_string()),
        &["c-a"],
        "regex freshsentinel",
    )?;
    // A shorter prefix of the same token, and an interior slice, both carry the
    // same fold partner and must behave identically.
    assert_ids(
        searcher.as_ref(),
        LqLeaf::Regex("fresh".to_string()),
        &["c-a"],
        "regex fresh",
    )?;
    assert_ids(
        searcher.as_ref(),
        LqLeaf::Regex("sentinel".to_string()),
        &["c-a"],
        "regex sentinel",
    )?;
    // A token with no fold partner beyond ASCII must keep working.
    assert_ids(
        searcher.as_ref(),
        LqLeaf::Regex("gamma".to_string()),
        &["c-a"],
        "regex gamma",
    )?;
    Ok(())
}

/// An explicit alternation must return documents holding either branch.
#[test]
fn alternation_regex_returns_documents_matching_any_branch() -> TestResult {
    let dir = tempfile::tempdir()?;
    let adapter = LexicalAdapter::with_state_root(dir.path().to_path_buf());
    build(
        &adapter,
        &[
            ("src/left.rs", "c-left", "alphamarker only here"),
            ("src/right.rs", "c-right", "betamarker only here"),
            ("src/both.rs", "c-both", "alphamarker and betamarker"),
            ("src/none.rs", "c-none", "neither token present"),
        ],
    )?;
    let searcher = open(&adapter)?;

    assert_ids(
        searcher.as_ref(),
        LqLeaf::Regex("(alphamarker|betamarker)".to_string()),
        &["c-both", "c-left", "c-right"],
        "regex alternation",
    )?;
    assert_ids(
        searcher.as_ref(),
        LqLeaf::Regex("alphamarker".to_string()),
        &["c-both", "c-left"],
        "regex left branch",
    )?;
    assert_ids(
        searcher.as_ref(),
        LqLeaf::Regex("zzzmissing".to_string()),
        &[],
        "regex absent token",
    )?;
    Ok(())
}
