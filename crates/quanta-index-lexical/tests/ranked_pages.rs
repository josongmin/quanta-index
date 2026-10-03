//! QI-BB-005 — ranked lexical pages are cut in the one total order and
//! continued by a cursor that neither repeats nor skips a row.
//!
//! The corpus is built so that the order matters where it used to break:
//! most chunks share their text, so their scores tie exactly, and the files
//! are written in reverse path order, so index order is the opposite of the
//! order the page promises. A collector that broke boundary ties by index
//! order (the old `TopDocs` cut) returns a first page that is not the
//! prefix of the full ranking.
//!
//! The oracles: the fixture itself for which chunks match and which files
//! hold them; the full ranking for the order every smaller page must
//! reproduce, checked to be strictly increasing in the row key; scores
//! equal exactly where the texts are equal.

#![forbid(unsafe_code)]

#[path = "support/source_fixture.rs"]
mod source_fixture;

use std::cmp::Ordering;
use std::collections::BTreeSet;
use std::error::Error;

use quanta_index_contract::lex::LanguageCode;
use quanta_index_contract::{
    ChunkId, ChunkRecord, LQ_VERSION_TAG, LexicalCandidate, LexicalCursor, LqCountBound, LqExpr,
    LqFilter, LqLeaf, LqOptions, LqQuery, LqSelect, LqSpan, ManifestGeneration,
    QueryConstraintSetV1, RepoId, RepoRelativePath, RevisionId, SearchCorpusIngestBatch,
    SearchCorpusReplaceScope,
};
use quanta_index_core::{
    CoreError, LexicalIndexOpenPort, LexicalPageSpec, LexicalSearcher, RequestBudgetV1,
    SearchCorpusBatchBuildPort,
};
use quanta_index_lexical::LexicalAdapter;

type TestResult = Result<(), Box<dyn Error>>;

/// Files in the corpus; each holds [`CHUNKS_PER_FILE`] chunks.
const FILES: u32 = 12;
const CHUNKS_PER_FILE: u32 = 3;

#[expect(
    clippy::expect_used,
    reason = "static fixture IDs provably satisfy the canonical ID policy"
)]
fn repo() -> RepoId {
    RepoId::new("ranked-repo").expect("static fixture ID satisfies canonical policy")
}

#[expect(
    clippy::expect_used,
    reason = "static fixture IDs provably satisfy the canonical ID policy"
)]
fn revision() -> RevisionId {
    RevisionId::new("ranked-rev").expect("static fixture ID satisfies canonical policy")
}

fn generation() -> ManifestGeneration {
    ManifestGeneration::new(1)
}

fn path(file: u32) -> String {
    format!("src/file_{file:02}.rs")
}

/// Chunk `chunk` of `file`: every third file's first chunk carries the
/// needle twice (a higher score); every other chunk the same text once
/// (a tie); chunk 2 of every file carries no needle at all.
fn chunk_text(file: u32, chunk: u32) -> String {
    match chunk {
        0 if file.is_multiple_of(3) => "fn lead() { rank_needle rank_needle }".to_string(),
        0 | 1 => "fn tied() { rank_needle filler }".to_string(),
        _ => "fn other() { unrelated filler }".to_string(),
    }
}

fn matches(chunk: u32) -> bool {
    chunk < 2
}

fn scope(file: u32) -> Result<SearchCorpusReplaceScope, Box<dyn Error>> {
    let language = LanguageCode::new("rust")
        .map_err(|err| -> Box<dyn Error> { format!("language code: {err}").into() })?;
    let mut raw_source = String::new();
    let chunks = (0..CHUNKS_PER_FILE)
        .map(|chunk| {
            let text = chunk_text(file, chunk);
            let start_byte = u32::try_from(raw_source.len())?;
            raw_source.push_str(&text);
            let end_byte = u32::try_from(raw_source.len())?;
            raw_source.push('\n');
            Ok(ChunkRecord {
                chunk_id: ChunkId::new(format!("chunk-{file:02}-{chunk}")),
                repo_relative_path: RepoRelativePath::new(path(file)),
                language: language.clone(),
                start_byte,
                end_byte,
                // Lines tie across files and differ within one.
                start_line: chunk.saturating_mul(10).saturating_add(1),
                end_line: chunk.saturating_mul(10).saturating_add(5),
                text: text.into_boxed_str(),
                structural: None,
                parent_chunk_id: None,
                source_repo_id: None,
            })
        })
        .collect::<Result<Vec<_>, Box<dyn Error>>>()?;
    Ok(source_fixture::complete_file(
        source_fixture::file_key(&repo(), &path(file)),
        &revision(),
        language,
        raw_source.as_bytes(),
        chunks,
        Vec::new(),
    )?)
}

/// The files are written in reverse path order, so the index order is the
/// opposite of the page order.
fn sealed_batch() -> Result<SearchCorpusIngestBatch, Box<dyn Error>> {
    Ok(source_fixture::sealed_batch(
        &repo(),
        &revision(),
        generation(),
        (0..FILES).rev().map(scope).collect::<Result<Vec<_>, _>>()?,
    )?)
}

fn query(filters: Vec<LqFilter>) -> LqQuery {
    LqQuery {
        lq_version: LQ_VERSION_TAG,
        expr: LqExpr::Leaf(LqLeaf::Keyword("rank_needle".to_string())),
        filters,
        options: LqOptions::defaults(),
        directives: Vec::new(),
        source_span: LqSpan::eof(0),
    }
}

fn searcher() -> Result<(tempfile::TempDir, Box<dyn LexicalSearcher>), Box<dyn Error>> {
    let dir = tempfile::tempdir()?;
    let adapter = LexicalAdapter::with_state_root(dir.path().to_path_buf());
    let _stages = adapter.build_batch(&sealed_batch()?)?;
    let searcher = adapter.open(
        &repo(),
        &revision(),
        generation(),
        &quanta_index_core::RequestBudgetV1::unbounded(),
    )?;
    Ok((dir, searcher))
}

fn page(
    searcher: &dyn LexicalSearcher,
    query: &LqQuery,
    fetch: u32,
    after: Option<LexicalCursor>,
) -> Result<quanta_index_core::LexicalSearchPageV1, CoreError> {
    searcher.search_constrained(
        query,
        &QueryConstraintSetV1::unconstrained(),
        &LexicalPageSpec { fetch, after },
        &RequestBudgetV1::unbounded(),
    )
}

fn ids(rows: &[LexicalCandidate]) -> Vec<String> {
    rows.iter().map(|row| row.candidate_id.clone()).collect()
}

fn cursor_at(row: &LexicalCandidate) -> LexicalCursor {
    LexicalCursor::at(generation(), row.order_key())
}

/// Walk every page of `size` rows through the cursors, collecting rows.
fn walk(
    searcher: &dyn LexicalSearcher,
    query: &LqQuery,
    size: u32,
) -> Result<Vec<LexicalCandidate>, Box<dyn Error>> {
    let mut rows: Vec<LexicalCandidate> = Vec::new();
    let mut after: Option<LexicalCursor> = None;
    loop {
        let next = page(searcher, query, size, after.clone())?.candidates;
        let Some(last) = next.last() else {
            return Ok(rows);
        };
        after = Some(cursor_at(last));
        rows.extend(next);
        if rows.len() > usize::try_from(FILES.saturating_mul(CHUNKS_PER_FILE))? {
            return Err("the walk returned more rows than the corpus holds".into());
        }
    }
}

/// The full ranking: every matching chunk once, strictly increasing in the
/// row key, equal texts at equal scores.
fn full_ranking(searcher: &dyn LexicalSearcher) -> Result<Vec<LexicalCandidate>, Box<dyn Error>> {
    let full = page(searcher, &query(Vec::new()), 1_000, None)?.candidates;
    let expected: BTreeSet<String> = (0..FILES)
        .flat_map(|file| {
            (0..CHUNKS_PER_FILE)
                .filter(|chunk| matches(*chunk))
                .map(move |chunk| format!("chunk-{file:02}-{chunk}"))
        })
        .collect();
    if ids(&full).into_iter().collect::<BTreeSet<_>>() != expected || full.len() != expected.len() {
        return Err(format!(
            "the full ranking is not the matching chunks: {:?}",
            ids(&full)
        )
        .into());
    }
    if full
        .iter()
        .zip(full.iter().skip(1))
        .any(|(left, right)| left.order_key().order(&right.order_key()) != Ordering::Less)
    {
        return Err(format!(
            "the full ranking is not in strict page order: {:?}",
            ids(&full)
        )
        .into());
    }
    // Every chunk that carries the needle once has the same text, so the
    // same score, bit for bit.
    let tied_ids: BTreeSet<String> = (0..FILES)
        .flat_map(|file| {
            (0..2_u32)
                .filter(move |chunk| *chunk == 1 || !file.is_multiple_of(3))
                .map(move |chunk| format!("chunk-{file:02}-{chunk}"))
        })
        .collect();
    let tied: BTreeSet<u32> = full
        .iter()
        .filter(|row| tied_ids.contains(&row.candidate_id))
        .map(|row| row.score.to_bits())
        .collect();
    if tied.len() != 1 {
        return Err(format!("equal texts must score exactly equal: {tied:?}").into());
    }
    Ok(full)
}

/// Every page size cuts the full ranking at its prefix — ties at the
/// boundary included — and the cursor walk reproduces it exactly.
#[test]
fn pages_are_prefixes_of_the_ranking_and_the_walk_reproduces_it() -> TestResult {
    let (_dir, searcher) = searcher()?;
    let full = full_ranking(searcher.as_ref())?;
    for size in [1_u32, 2, 3, 5, 7, 16] {
        let first = page(searcher.as_ref(), &query(Vec::new()), size, None)?.candidates;
        let prefix: Vec<LexicalCandidate> =
            full.iter().take(usize::try_from(size)?).cloned().collect();
        if ids(&first) != ids(&prefix) {
            return Err(format!(
                "a first page of {size} is not the ranking's prefix: {:?} vs {:?}",
                ids(&first),
                ids(&prefix)
            )
            .into());
        }
        let walked = walk(searcher.as_ref(), &query(Vec::new()), size)?;
        if ids(&walked) != ids(&full) {
            return Err(format!(
                "the {size}-row walk drifted from the ranking: {:?}",
                ids(&walked)
            )
            .into());
        }
    }
    Ok(())
}

/// A counted page after a cursor counts exactly the rows after it.
#[test]
fn a_counted_page_counts_the_rows_after_its_cursor() -> TestResult {
    let (_dir, searcher) = searcher()?;
    let full = full_ranking(searcher.as_ref())?;
    let mut counted = query(Vec::new());
    counted.options.count = Some(LqCountBound::All);
    for (position, row) in full.iter().enumerate() {
        let after = page(searcher.as_ref(), &counted, 2, Some(cursor_at(row)))?;
        let remaining = u64::try_from(full.len().saturating_sub(position.saturating_add(1)))?;
        if after.exact_total != Some(remaining) {
            return Err(format!(
                "after row {position} the count is {:?}, {remaining} rows remain",
                after.exact_total
            )
            .into());
        }
    }
    Ok(())
}

/// A boosted query ranks identically: the collector that cannot skip
/// blocks (every score scaled) agrees with the one that does.
#[test]
fn a_boosted_ranking_is_the_same_order() -> TestResult {
    let (_dir, searcher) = searcher()?;
    let full = full_ranking(searcher.as_ref())?;
    let mut boosted = query(Vec::new());
    boosted.options.boost_millis = Some(2_000);
    let ranked = page(searcher.as_ref(), &boosted, 1_000, None)?.candidates;
    if ids(&ranked) != ids(&full) {
        return Err(format!("the boosted ranking drifted: {:?}", ids(&ranked)).into());
    }
    let walked = walk(searcher.as_ref(), &boosted, 4)?;
    if ids(&walked) != ids(&full) {
        return Err(format!("the boosted walk drifted: {:?}", ids(&walked)).into());
    }
    Ok(())
}

/// A path projection keeps each file's first row of the ranking, pages
/// through them with the same cursor, and totals the files after it.
#[test]
fn a_projection_pages_through_each_files_first_row() -> TestResult {
    let (_dir, searcher) = searcher()?;
    let full = full_ranking(searcher.as_ref())?;
    let mut seen: BTreeSet<String> = BTreeSet::new();
    let representatives: Vec<LexicalCandidate> = full
        .iter()
        .filter(|row| seen.insert(row.repo_relative_path.as_str().to_string()))
        .cloned()
        .collect();
    if representatives.len() != usize::try_from(FILES)? {
        return Err("every file holds a match".into());
    }
    let projected = query(vec![LqFilter::Select {
        dim: LqSelect::Path,
    }]);
    let whole = page(searcher.as_ref(), &projected, 1_000, None)?;
    if ids(&whole.candidates) != ids(&representatives)
        || whole.exact_total != Some(u64::from(FILES))
    {
        return Err(format!(
            "the projection is not each file's first row: {:?} total {:?}",
            ids(&whole.candidates),
            whole.exact_total
        )
        .into());
    }
    let walked = walk(searcher.as_ref(), &projected, 5)?;
    if ids(&walked) != ids(&representatives) {
        return Err(format!("the projected walk drifted: {:?}", ids(&walked)).into());
    }
    let Some(fourth) = representatives.get(3) else {
        return Err("twelve representatives".into());
    };
    let after = page(searcher.as_ref(), &projected, 2, Some(cursor_at(fourth)))?;
    if after.exact_total != Some(u64::from(FILES).saturating_sub(4)) {
        return Err(format!(
            "the projected total after four files: {:?}",
            after.exact_total
        )
        .into());
    }
    Ok(())
}

/// A cursor cut from another generation is refused typed, never ranked
/// against scores it did not come from.
#[test]
fn a_cursor_from_another_generation_is_refused() -> TestResult {
    let (_dir, searcher) = searcher()?;
    let full = full_ranking(searcher.as_ref())?;
    let Some(first) = full.first() else {
        return Err("a ranking".into());
    };
    let mut foreign = cursor_at(first);
    foreign.manifest_generation = ManifestGeneration::new(2);
    match page(searcher.as_ref(), &query(Vec::new()), 3, Some(foreign)) {
        Err(CoreError::Typed {
            code: quanta_index_contract::SearchPlaneErrorCodeV2::QueryCursorGenerationMismatch,
            ..
        }) => Ok(()),
        other => Err(format!("a foreign cursor must be refused typed: {other:?}").into()),
    }
}
