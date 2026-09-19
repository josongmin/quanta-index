//! QI-BB-005 보완 #4 — the ranked lexical page's cursor on the wire.
//!
//! A text or symbol page continues exactly when its window says more rows
//! exist, and then its cursor names its last row in its own generation;
//! its rows are in strict page order. The decoder holds every page to that
//! fail-closed, because a page that breaks it would make a client skip or
//! repeat rows. The oracle for every refusal is a page the encoder writes
//! as given (encoding checks nothing but the cursor's score) and the
//! decoder must refuse.

#![forbid(unsafe_code)]

use std::error::Error;

use quanta_index_contract::lex::SymbolKindCode;
use quanta_index_contract::{
    CandidateCountV1, FileOwnerProjectionRow, GenerationPin, LexicalCandidate, LexicalCursorV1,
    ManifestGeneration, QueryConstraintSetV1, QueryResultWindowV1, RepoId, RepoRelativePath,
    RevisionId, SymbolCandidate, SymbolQueryRequest, SymbolQueryResponse, TextQueryRequest,
    TextQueryResponse, TextQuerySyntax,
};

type TestResult = Result<(), Box<dyn Error>>;

fn pin() -> GenerationPin {
    GenerationPin::new(
        RepoId::new("repo-1"),
        RevisionId::new("rev-1"),
        ManifestGeneration::new(7),
    )
}

fn row(id: &str, score: f32, path: &str, line: u32) -> LexicalCandidate {
    LexicalCandidate {
        candidate_id: id.to_string(),
        repo_id: RepoId::new("repo-1"),
        revision_id: RevisionId::new("rev-1"),
        manifest_generation: ManifestGeneration::new(7),
        repo_relative_path: RepoRelativePath::new(path),
        start_line: line,
        end_line: line,
        score,
        snippet: "fn x() {}".to_string(),
        snippet_hit_offset: None,
        highlights: Vec::new(),
    }
}

/// Two rows in page order: the higher score first, then by path on a tie.
fn rows() -> Vec<LexicalCandidate> {
    vec![
        row("b", 2.0, "src/a.rs", 1),
        row("a", 1.0, "src/a.rs", 1),
        row("c", 1.0, "src/b.rs", 1),
    ]
}

fn cursor_at(row: &LexicalCandidate) -> LexicalCursorV1 {
    LexicalCursorV1::at(ManifestGeneration::new(7), row.order_key())
}

fn continued(results: Vec<LexicalCandidate>) -> Result<TextQueryResponse, Box<dyn Error>> {
    let last = results.last().ok_or("rows")?;
    Ok(TextQueryResponse {
        generation: pin(),
        next_cursor: Some(cursor_at(last)),
        window: QueryResultWindowV1::new(
            u32::try_from(results.len())?,
            CandidateCountV1::AtLeast(u64::try_from(results.len())?.saturating_add(1)),
            true,
        )?,
        results,
        file_owner_rows: None,
    })
}

fn round_trip_text(response: &TextQueryResponse) -> Result<TextQueryResponse, Box<dyn Error>> {
    let mut bytes = Vec::new();
    ciborium::into_writer(response, &mut bytes)?;
    Ok(ciborium::from_reader(bytes.as_slice())?)
}

fn refused_text(response: &TextQueryResponse, why: &str) -> TestResult {
    let mut bytes = Vec::new();
    ciborium::into_writer(response, &mut bytes)?;
    ciborium::from_reader::<TextQueryResponse, _>(bytes.as_slice()).map_or_else(
        |_refused| Ok(()),
        |decoded| Err(format!("{why}: decoded {decoded:?}").into()),
    )
}

#[test]
fn a_continued_page_and_a_final_page_round_trip() -> TestResult {
    let page = continued(rows())?;
    if round_trip_text(&page)? != page {
        return Err("a continued page must round-trip".into());
    }
    let last_page = TextQueryResponse {
        generation: pin(),
        results: rows(),
        window: QueryResultWindowV1::exact(3),
        file_owner_rows: None,
        next_cursor: None,
    };
    if round_trip_text(&last_page)? != last_page {
        return Err("a final page must round-trip".into());
    }
    Ok(())
}

#[test]
fn the_decoder_refuses_every_page_that_would_skip_or_repeat() -> TestResult {
    let mut no_cursor = continued(rows())?;
    no_cursor.next_cursor = None;
    refused_text(&no_cursor, "more rows without a cursor")?;

    let mut stray_cursor = continued(rows())?;
    stray_cursor.window = QueryResultWindowV1::exact(3);
    refused_text(&stray_cursor, "a cursor on the last page")?;

    let mut not_last = continued(rows())?;
    not_last.next_cursor = rows().first().map(cursor_at);
    refused_text(&not_last, "a cursor that is not the page's last row")?;

    let mut foreign = continued(rows())?;
    if let Some(cursor) = foreign.next_cursor.as_mut() {
        cursor.manifest_generation = ManifestGeneration::new(8);
    }
    refused_text(&foreign, "a cursor from another generation")?;

    let mut shuffled = rows();
    shuffled.swap(0, 1);
    refused_text(&continued(shuffled)?, "rows out of page order")?;

    let mut duplicated = rows();
    duplicated.push(row("c", 1.0, "src/b.rs", 1));
    refused_text(&continued(duplicated)?, "a repeated row")?;

    let mut owners = continued(rows())?;
    owners.file_owner_rows = Some(vec![FileOwnerProjectionRow {
        candidate_id: "b".to_string(),
        repo_id: RepoId::new("repo-1"),
        revision_id: RevisionId::new("rev-1"),
        manifest_generation: ManifestGeneration::new(7),
        repo_relative_path: RepoRelativePath::new("src/a.rs"),
        owners: Vec::new(),
    }]);
    refused_text(&owners, "owner rows that do not pair with the results")
}

#[test]
fn a_cursor_score_must_be_finite_on_both_sides() -> TestResult {
    let mut cursor = rows().first().map(cursor_at).ok_or("a row")?;
    cursor.score = f32::NAN;
    let mut bytes = Vec::new();
    if ciborium::into_writer(&cursor, &mut bytes).is_ok() {
        return Err("a non-finite cursor must not encode".into());
    }
    // Raw bytes carrying one anyway are refused on decode.
    let mut finite = rows().first().map(cursor_at).ok_or("a row")?;
    finite.score = 1.5;
    let mut value = ciborium::Value::serialized(&finite)?;
    if let ciborium::Value::Map(entries) = &mut value {
        for (key, entry) in entries.iter_mut() {
            if key.as_text() == Some("score") {
                *entry = ciborium::Value::Float(f64::INFINITY);
            }
        }
    }
    let mut raw = Vec::new();
    ciborium::into_writer(&value, &mut raw)?;
    ciborium::from_reader::<LexicalCursorV1, _>(raw.as_slice()).map_or_else(
        |_refused| Ok(()),
        |decoded| Err(format!("an infinite score decoded: {decoded:?}").into()),
    )
}

/// The symbol request is the text request's wire shape, cursor included,
/// and the symbol page obeys the same continuation rule.
#[test]
fn symbol_requests_and_pages_share_the_text_rules() -> TestResult {
    let request = SymbolQueryRequest {
        syntax: TextQuerySyntax::Native,
        query_text: "needle".to_string(),
        constraints: QueryConstraintSetV1::unconstrained(),
        generation: Some(pin()),
        generation_selector: None,
        top_k: 3,
        cursor: rows().last().map(cursor_at),
    };
    let mut bytes = Vec::new();
    ciborium::into_writer(&request, &mut bytes)?;
    let as_symbol: SymbolQueryRequest = ciborium::from_reader(bytes.as_slice())?;
    let as_text: TextQueryRequest = ciborium::from_reader(bytes.as_slice())?;
    if as_symbol != request || as_text != TextQueryRequest::from(request.clone()) {
        return Err("the symbol request must be the text request's shape".into());
    }

    let symbol = |row: &LexicalCandidate| -> Result<SymbolCandidate, Box<dyn Error>> {
        Ok(SymbolCandidate {
            candidate_id: row.candidate_id.clone(),
            repo_id: row.repo_id.clone(),
            revision_id: row.revision_id.clone(),
            manifest_generation: row.manifest_generation,
            repo_relative_path: row.repo_relative_path.clone(),
            start_line: row.start_line,
            end_line: row.end_line,
            score: row.score,
            snippet: row.snippet.clone(),
            symbol_kind: SymbolKindCode::new("function")?,
            symbol_kind_family: None,
        })
    };
    let results = rows().iter().map(symbol).collect::<Result<Vec<_>, _>>()?;
    let page = SymbolQueryResponse {
        generation: pin(),
        next_cursor: rows().last().map(cursor_at),
        window: QueryResultWindowV1::new(3, CandidateCountV1::AtLeast(4), true)?,
        results,
    };
    let mut bytes = Vec::new();
    ciborium::into_writer(&page, &mut bytes)?;
    let decoded: SymbolQueryResponse = ciborium::from_reader(bytes.as_slice())?;
    if decoded != page {
        return Err("a continued symbol page must round-trip".into());
    }
    let mut stray = page;
    stray.next_cursor = None;
    let mut bytes = Vec::new();
    ciborium::into_writer(&stray, &mut bytes)?;
    ciborium::from_reader::<SymbolQueryResponse, _>(bytes.as_slice()).map_or_else(
        |_refused| Ok(()),
        |decoded| {
            Err(format!("a symbol page with more rows and no cursor decoded: {decoded:?}").into())
        },
    )
}
