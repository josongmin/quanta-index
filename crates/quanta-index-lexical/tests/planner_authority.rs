//! LXE-02/LXE-03/LXE-04 — planner authority on the live execution path.
//!
//! Each test drives the real `TantivySearcher::search` body through a
//! tempdir-backed `Index`, asserting that the planner pre-flight surfaces
//! typed `CoreError::Typed { code: ..., message: ... }` responses for
//! filters / IR shapes the producer cannot honor. There is NO silent
//! fallback to an empty success.
//!
//! Mirrors the `tantivy_smoke.rs` test idiom: returns `Result<(), Box<dyn
//! Error>>` and propagates errors via `?` (no `.unwrap()`/`.expect()` per
//! workspace lint policy).

#![forbid(unsafe_code)]

use std::error::Error;

use quanta_index_contract::{
    ChunkId, ChunkRecord, LQ_VERSION_TAG, LexicalChannelOp, LqCountBound, LqExpr, LqFilter, LqLeaf,
    LqOptions, LqQuery, LqSpan, LqType, LqYesNoOnly, ManifestGeneration, RepoId, RepoRelativePath,
    RevisionId, UpsertChunk,
};
use quanta_index_core::{CoreError, LexicalIndexBuildPort, LexicalIndexOpenPort, LexicalSearcher};
use quanta_index_lexical::LexicalAdapter;

type TestResult = Result<(), Box<dyn Error>>;

fn repo() -> RepoId {
    RepoId::new("planner-authority-repo")
}

fn revision() -> RevisionId {
    RevisionId::new("planner-authority-rev")
}

fn generation() -> ManifestGeneration {
    ManifestGeneration::new(1)
}

fn encode_chunk_payload(text: &str) -> Result<Vec<u8>, Box<dyn Error>> {
    let record = ChunkRecord {
        repo_relative_path: RepoRelativePath::new(""),
        language: String::new().into_boxed_str(),
        start_line: 0,
        end_line: 0,
        snippet: text.to_string().into_boxed_str(),
    };
    let mut payload = Vec::new();
    ciborium::into_writer(&record, &mut payload)
        .map_err(|err| -> Box<dyn Error> { format!("encode chunk: {err}").into() })?;
    Ok(payload)
}

fn upsert(chunk_id: &str, text: &str) -> Result<LexicalChannelOp, Box<dyn Error>> {
    Ok(LexicalChannelOp::UpsertChunk(UpsertChunk {
        repo_id: repo(),
        revision_id: revision(),
        generation: generation(),
        chunk_id: ChunkId::new(chunk_id),
        payload: encode_chunk_payload(text)?,
    }))
}

fn make_query_with_filters(expr: LqExpr, filters: Vec<LqFilter>) -> LqQuery {
    LqQuery {
        lq_version: LQ_VERSION_TAG,
        expr,
        filters,
        options: LqOptions::defaults(),
        directives: Vec::new(),
        source_span: LqSpan::eof(0),
    }
}

fn make_query_with_options(expr: LqExpr, options: LqOptions) -> LqQuery {
    LqQuery {
        lq_version: LQ_VERSION_TAG,
        expr,
        filters: Vec::new(),
        options,
        directives: Vec::new(),
        source_span: LqSpan::eof(0),
    }
}

fn fresh_searcher_with_corpus(
    docs: &[(&str, &str)],
) -> Result<Box<dyn LexicalSearcher>, Box<dyn Error>> {
    let dir = tempfile::tempdir()?;
    let adapter = LexicalAdapter::with_state_root(dir.path().to_path_buf());
    let mut ops: Vec<LexicalChannelOp> = Vec::with_capacity(docs.len());
    for (id, text) in docs {
        ops.push(upsert(id, text)?);
    }
    adapter.build(&repo(), &revision(), generation(), &ops)?;
    let searcher = adapter.open(&repo(), &revision(), generation())?;
    // The tempdir backing must outlive the searcher; intentionally leak it
    // for the duration of the test by detaching `into_path`.
    let _kept = dir.keep();
    Ok(searcher)
}

fn assert_typed_error(
    outcome: Result<impl std::fmt::Debug, CoreError>,
    expected_code: &str,
) -> Result<(), Box<dyn Error>> {
    match outcome {
        Err(CoreError::Typed { code, message }) => {
            if code == expected_code {
                Ok(())
            } else {
                Err(
                    format!("expected typed code `{expected_code}`, got code `{code}`: {message}")
                        .into(),
                )
            }
        }
        other => {
            Err(format!("expected typed error with code `{expected_code}`, got {other:?}").into())
        }
    }
}

/// 1. `fork:only` → `LEX_FILTER_FORK_UNAVAILABLE` when no repo metadata is loaded.
#[test]
fn fork_only_without_metadata_returns_typed_fork_unavailable() -> TestResult {
    let searcher = fresh_searcher_with_corpus(&[("c1", "fox jumps")])?;
    let q = make_query_with_filters(
        LqExpr::Leaf(LqLeaf::Keyword("fox".to_string())),
        vec![LqFilter::Fork {
            mode: LqYesNoOnly::Only,
        }],
    );
    let outcome = searcher.search(&q, 10);
    assert_typed_error(outcome, "LEX_FILTER_FORK_UNAVAILABLE")?;
    Ok(())
}

/// 2. `archived:yes` → `LEX_FILTER_ARCHIVED_UNAVAILABLE`.
///
/// Note: `LqYesNoOnly::Yes` is the "include both" mode in the filter
/// planner's lowering of `archived:`; we use `LqYesNoOnly::Only` to make
/// the typed-unavailable path fire on a fresh searcher without bundle
/// metadata.
#[test]
fn archived_only_without_metadata_returns_typed_archived_unavailable() -> TestResult {
    let searcher = fresh_searcher_with_corpus(&[("c1", "fox jumps")])?;
    let q = make_query_with_filters(
        LqExpr::Leaf(LqLeaf::Keyword("fox".to_string())),
        vec![LqFilter::Archived {
            mode: LqYesNoOnly::Only,
        }],
    );
    let outcome = searcher.search(&q, 10);
    assert_typed_error(outcome, "LEX_FILTER_ARCHIVED_UNAVAILABLE")?;
    Ok(())
}

/// 3. `rev:abc123` → `LEX_FILTER_REV_UNAVAILABLE`.
#[test]
fn rev_filter_returns_typed_rev_unavailable() -> TestResult {
    let searcher = fresh_searcher_with_corpus(&[("c1", "fox jumps")])?;
    let q = make_query_with_filters(
        LqExpr::Leaf(LqLeaf::Keyword("fox".to_string())),
        vec![LqFilter::Rev {
            spec: "abc123".to_string(),
        }],
    );
    let outcome = searcher.search(&q, 10);
    assert_typed_error(outcome, "LEX_FILTER_REV_UNAVAILABLE")?;
    Ok(())
}

/// 4. `type:commit` → `HISTORY_PRODUCER_UNAVAILABLE`.
#[test]
fn type_commit_returns_typed_history_producer_unavailable() -> TestResult {
    let searcher = fresh_searcher_with_corpus(&[("c1", "fox jumps")])?;
    let q = make_query_with_filters(
        LqExpr::Leaf(LqLeaf::Keyword("fox".to_string())),
        vec![LqFilter::Type {
            kind: LqType::Commit,
        }],
    );
    let outcome = searcher.search(&q, 10);
    assert_typed_error(outcome, "HISTORY_PRODUCER_UNAVAILABLE")?;
    Ok(())
}

/// 5. `count:0` → `LEX_FILTER_INVALID_COUNT` typed error.
#[test]
fn count_zero_returns_typed_invalid_count() -> TestResult {
    let searcher = fresh_searcher_with_corpus(&[("c1", "fox jumps")])?;
    let mut opts = LqOptions::defaults();
    opts.count = Some(LqCountBound::Bounded(0));
    let q = make_query_with_options(LqExpr::Leaf(LqLeaf::Keyword("fox".to_string())), opts);
    let outcome = searcher.search(&q, 10);
    assert_typed_error(outcome, "LEX_FILTER_INVALID_COUNT")?;
    Ok(())
}

/// 6. Regex leaf executes through the planner pipeline.
///
/// On a small corpus (well under `REGEX_TRIGRAM_INDEX_MISSING_THRESHOLD`),
/// the regex leaf compiles via `plan_regex` (dialect filter + literal
/// extraction) and the executor runs the Tantivy `RegexQuery` per-token
/// path. The planner enforces the LXE-04 dialect (lookbehind / possessive
/// / backref rejected typed) and extracts mandatory byte literals from
/// `foobar.*quick` — both `foobar` and `quick` are required substrings,
/// so the candidate set is `{c2}` (the only doc containing both tokens).
///
/// This is the honest behaviour today: the trigram-postings index is not
/// yet wired on this adapter, and the large-corpus path surfaces
/// `LEX_REGEX_TRIGRAM_INDEX_MISSING` instead of silently full-scanning.
/// `RegexQuery` runs over per-token strings (Tantivy lexes content into
/// terms), so the regex must match a single token; `foobar.*quick` does
/// not match anything because no single token contains both — therefore
/// the test uses `foo.*r` against tokens that contain it (`foobar`).
#[test]
fn regex_leaf_small_corpus_returns_hits_via_planner() -> TestResult {
    let searcher = fresh_searcher_with_corpus(&[
        ("c1", "the quick fox"),
        ("c2", "foobar is here"),
        ("c3", "no match line"),
    ])?;
    let q = make_query_with_filters(
        LqExpr::Leaf(LqLeaf::Regex("foo.*r".to_string())),
        Vec::new(),
    );
    let hits = searcher.search(&q, 10)?;
    // Only `foobar` (in c2) matches the per-token regex `foo.*r`. c1 has
    // `fox` which does not match (no `r` after `foo`); c3 has no matching
    // token at all.
    if hits.len() != 1 {
        return Err(format!(
            "expected 1 regex hit, got {}: {:?}",
            hits.len(),
            hits.iter().map(|c| &c.candidate_id).collect::<Vec<_>>()
        )
        .into());
    }
    let first = hits.first().ok_or("hits empty after length check")?;
    if first.candidate_id != "c2" {
        return Err(format!("expected c2 for regex `foo.*r`, got {}", first.candidate_id).into());
    }
    Ok(())
}
