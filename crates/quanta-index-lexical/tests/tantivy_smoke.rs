//! Smoke test for the Tantivy-backed lexical adapter.
//!
//! Builds an index from a sealed batch of `UpsertChunk` ops, opens a searcher,
//! and verifies BM25 scoring + boolean composition both return the expected
//! candidate sets. Follows the `wal_roundtrip.rs` test idiom: returns
//! `Result<(), Box<dyn Error>>` and propagates errors via `?` (no `.unwrap()`
//! or `.expect()` per the workspace lint policy).

#![forbid(unsafe_code)]

use std::error::Error;

use quanta_index_contract::channel::LexicalChannelOp;
use quanta_index_contract::lex::{
    LanguageCode, SymbolKindCode, SymbolKindFamily, SymbolRecord, SymbolRelationship, SymbolSpan,
};
use quanta_index_contract::{
    ChunkId, ChunkRecord, LQ_VERSION_TAG, LexicalFullBundle, LqCase, LqCountBound, LqExpr,
    LqFileScope, LqFilter, LqLeaf, LqOptions, LqPatternType, LqPredicateArg, LqQuery, LqSelect,
    LqSpan, LqType, LqVisibility, LqYesNoOnly, ManifestGeneration, RepoId, RepoRelativePath,
    RevisionId, SymbolId, UpsertChunk, UpsertSymbol,
};
use quanta_index_core::{CoreError, LexicalIndexBuildPort, LexicalIndexOpenPort};
use quanta_index_lexical::{LEXICAL_WRITER_CACHE_MAX, LexicalAdapter};

type TestResult = Result<(), Box<dyn Error>>;

#[derive(Clone, Debug)]
struct RepoMetadataPayload {
    fork: bool,
    archived: bool,
    visibility: LqVisibility,
    contexts: Vec<String>,
}

fn repo() -> RepoId {
    RepoId::new("smoke-repo")
}

fn revision() -> RevisionId {
    RevisionId::new("smoke-rev")
}

fn generation() -> ManifestGeneration {
    ManifestGeneration::new(1)
}

fn language_code(code: &str) -> Result<LanguageCode, Box<dyn Error>> {
    LanguageCode::new(code).map_err(|err| -> Box<dyn Error> {
        format!("invalid language code `{code}`: {err}").into()
    })
}

fn encode_chunk_payload(chunk_id: &str, text: &str) -> Result<Vec<u8>, Box<dyn Error>> {
    encode_chunk_payload_with_metadata(chunk_id, "", "", 0, 0, text)
}

fn encode_chunk_payload_with_metadata(
    chunk_id: &str,
    repo_relative_path: &str,
    language: &str,
    start_line: u32,
    end_line: u32,
    text: &str,
) -> Result<Vec<u8>, Box<dyn Error>> {
    encode_chunk_payload_with_texts(
        chunk_id,
        repo_relative_path,
        language,
        start_line,
        end_line,
        text,
        text,
    )
}

fn encode_chunk_payload_with_texts(
    chunk_id: &str,
    repo_relative_path: &str,
    language: &str,
    start_line: u32,
    end_line: u32,
    _snippet: &str,
    indexed_text: &str,
) -> Result<Vec<u8>, Box<dyn Error>> {
    let repo_relative_path = if repo_relative_path.is_empty() {
        "src/smoke.txt"
    } else {
        repo_relative_path
    };
    let language = if language.is_empty() {
        "text"
    } else {
        language
    };
    let record = ChunkRecord {
        chunk_id: ChunkId::new(chunk_id),
        repo_relative_path: RepoRelativePath::new(repo_relative_path),
        language: language_code(language)?,
        start_byte: 0,
        end_byte: u32::try_from(indexed_text.len()).map_err(|err| -> Box<dyn Error> {
            format!("chunk text length overflow: {err}").into()
        })?,
        start_line,
        end_line,
        text: indexed_text.to_string().into_boxed_str(),
        structural: None,
        parent_chunk_id: None,
        source_repo_id: None,
    };
    let mut payload = Vec::new();
    ciborium::into_writer(&record, &mut payload)
        .map_err(|err| -> Box<dyn Error> { format!("encode chunk: {err}").into() })?;
    Ok(payload)
}

fn encode_repo_metadata_payload(
    fork: bool,
    archived: bool,
    visibility: LqVisibility,
    contexts: &[&str],
) -> Result<Vec<u8>, Box<dyn Error>> {
    let record = RepoMetadataPayload {
        fork,
        archived,
        visibility,
        contexts: contexts.iter().map(ToString::to_string).collect(),
    };
    let mut payload = Vec::new();
    let mut visibility_payload = Vec::new();
    ciborium::into_writer(&record.visibility, &mut visibility_payload).map_err(
        |err| -> Box<dyn Error> { format!("encode repo metadata visibility: {err}").into() },
    )?;
    let visibility_wire: ciborium::Value = ciborium::from_reader(visibility_payload.as_slice())
        .map_err(|err| -> Box<dyn Error> {
            format!("decode repo metadata visibility wire: {err}").into()
        })?;
    let wire = ciborium::Value::Map(vec![
        (
            ciborium::Value::Text("fork".to_string()),
            ciborium::Value::Bool(record.fork),
        ),
        (
            ciborium::Value::Text("archived".to_string()),
            ciborium::Value::Bool(record.archived),
        ),
        (
            ciborium::Value::Text("visibility".to_string()),
            visibility_wire,
        ),
        (
            ciborium::Value::Text("contexts".to_string()),
            ciborium::Value::Array(
                record
                    .contexts
                    .iter()
                    .cloned()
                    .map(ciborium::Value::Text)
                    .collect(),
            ),
        ),
    ]);
    ciborium::into_writer(&wire, &mut payload)
        .map_err(|err| -> Box<dyn Error> { format!("encode repo metadata: {err}").into() })?;
    Ok(payload)
}

fn encode_symbol_payload(
    symbol_id: &str,
    path: &str,
    language: &str,
    name: &str,
    line_start: u32,
    line_end: u32,
) -> Result<Vec<u8>, Box<dyn Error>> {
    let record = SymbolRecord {
        symbol_id: SymbolId::new(symbol_id),
        repo_relative_path: RepoRelativePath::new(path),
        language: language_code(language)?,
        symbol_kind: SymbolKindCode::new("function").map_err(|err| -> Box<dyn Error> {
            format!("invalid symbol kind code: {err}").into()
        })?,
        symbol_kind_family: Some(SymbolKindFamily::Callable),
        local_name: name.into(),
        qualified_name: format!("crate::{name}").into_boxed_str(),
        signature: None,
        visibility: None,
        definition_span: SymbolSpan {
            path: path.into(),
            byte_start: 0,
            byte_end: 8,
            line_start,
            line_end,
        },
        container_qualified_name: None,
        relationship: SymbolRelationship::Def,
    };
    let mut payload = Vec::new();
    ciborium::into_writer(&record, &mut payload)
        .map_err(|err| -> Box<dyn Error> { format!("encode symbol: {err}").into() })?;
    Ok(payload)
}

fn upsert(chunk_id: &str, text: &str) -> Result<LexicalChannelOp, Box<dyn Error>> {
    upsert_with_metadata(chunk_id, "", "", 0, 0, text)
}

fn upsert_with_metadata(
    chunk_id: &str,
    repo_relative_path: &str,
    language: &str,
    start_line: u32,
    end_line: u32,
    text: &str,
) -> Result<LexicalChannelOp, Box<dyn Error>> {
    Ok(LexicalChannelOp::UpsertChunk(UpsertChunk {
        repo_id: repo(),
        revision_id: revision(),
        generation: generation(),
        chunk_id: ChunkId::new(chunk_id),
        payload: encode_chunk_payload_with_metadata(
            chunk_id,
            repo_relative_path,
            language,
            start_line,
            end_line,
            text,
        )?,
    }))
}

fn upsert_with_source_repo(
    chunk_id: &str,
    repo_relative_path: &str,
    language: &str,
    start_line: u32,
    end_line: u32,
    text: &str,
    source_repo_id: &str,
) -> Result<LexicalChannelOp, Box<dyn Error>> {
    let record = ChunkRecord {
        chunk_id: ChunkId::new(chunk_id),
        repo_relative_path: RepoRelativePath::new(repo_relative_path),
        language: language_code(language)?,
        start_byte: 0,
        end_byte: u32::try_from(text.len()).map_err(|err| -> Box<dyn Error> {
            format!("chunk text length overflow: {err}").into()
        })?,
        start_line,
        end_line,
        text: text.to_string().into_boxed_str(),
        structural: None,
        parent_chunk_id: None,
        source_repo_id: Some(RepoId::new(source_repo_id)),
    };
    let mut payload = Vec::new();
    ciborium::into_writer(&record, &mut payload)
        .map_err(|err| -> Box<dyn Error> { format!("encode chunk: {err}").into() })?;
    Ok(LexicalChannelOp::UpsertChunk(UpsertChunk {
        repo_id: repo(),
        revision_id: revision(),
        generation: generation(),
        chunk_id: ChunkId::new(chunk_id),
        payload,
    }))
}

fn upsert_with_texts(
    chunk_id: &str,
    repo_relative_path: &str,
    language: &str,
    start_line: u32,
    end_line: u32,
    snippet: &str,
    indexed_text: &str,
) -> Result<LexicalChannelOp, Box<dyn Error>> {
    Ok(LexicalChannelOp::UpsertChunk(UpsertChunk {
        repo_id: repo(),
        revision_id: revision(),
        generation: generation(),
        chunk_id: ChunkId::new(chunk_id),
        payload: encode_chunk_payload_with_texts(
            chunk_id,
            repo_relative_path,
            language,
            start_line,
            end_line,
            snippet,
            indexed_text,
        )?,
    }))
}

fn upsert_symbol(
    symbol_id: &str,
    path: &str,
    language: &str,
    name: &str,
    line_start: u32,
    line_end: u32,
) -> Result<LexicalChannelOp, Box<dyn Error>> {
    Ok(LexicalChannelOp::UpsertSymbol(UpsertSymbol {
        repo_id: repo(),
        revision_id: revision(),
        generation: generation(),
        symbol_id: SymbolId::new(symbol_id),
        payload: encode_symbol_payload(symbol_id, path, language, name, line_start, line_end)?,
    }))
}

fn make_query(expr: LqExpr) -> LqQuery {
    make_query_with_filters(expr, Vec::new())
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

#[test]
fn tantivy_index_round_trip() -> TestResult {
    let dir = tempfile::tempdir()?;
    let adapter = LexicalAdapter::with_state_root(dir.path().to_path_buf());

    let ops = vec![
        upsert("c1", "fox jumps")?,
        upsert("c2", "lazy dog")?,
        upsert("c3", "fox is quick")?,
    ];
    adapter.build(&repo(), &revision(), generation(), &ops)?;

    let searcher = adapter.open(&repo(), &revision(), generation())?;

    // 1) "fox" -> expects c1 and c3 (both contain "fox").
    let fox_hits = searcher.search(
        &make_query(LqExpr::Leaf(LqLeaf::Keyword("fox".to_string()))),
        10,
    )?;
    if fox_hits.len() != 2 {
        return Err(format!(
            "expected 2 candidates for `fox`, got {}: {:?}",
            fox_hits.len(),
            fox_hits.iter().map(|c| &c.candidate_id).collect::<Vec<_>>()
        )
        .into());
    }
    let mut fox_ids: Vec<String> = fox_hits.iter().map(|c| c.candidate_id.clone()).collect();
    fox_ids.sort();
    if fox_ids != vec!["c1".to_string(), "c3".to_string()] {
        return Err(format!("expected ids [c1, c3] for `fox`, got {fox_ids:?}").into());
    }

    // 2) "lazy" -> expects only c2.
    let lazy_hits = searcher.search(
        &make_query(LqExpr::Leaf(LqLeaf::Keyword("lazy".to_string()))),
        10,
    )?;
    if lazy_hits.len() != 1 {
        return Err(format!("expected 1 candidate for `lazy`, got {}", lazy_hits.len()).into());
    }
    let first_lazy = lazy_hits
        .first()
        .ok_or("lazy hits empty after length check")?;
    if first_lazy.candidate_id != "c2" {
        return Err(format!("expected id c2 for `lazy`, got {}", first_lazy.candidate_id).into());
    }

    // 3) All([Raw("fox"), Raw("quick")]) -> expects only c3.
    let all_expr = LqExpr::All(vec![
        LqExpr::Leaf(LqLeaf::Keyword("fox".to_string())),
        LqExpr::Leaf(LqLeaf::Keyword("quick".to_string())),
    ]);
    let all_hits = searcher.search(&make_query(all_expr), 10)?;
    if all_hits.len() != 1 {
        return Err(format!(
            "expected 1 candidate for All([fox, quick]), got {}",
            all_hits.len()
        )
        .into());
    }
    let first_all = all_hits
        .first()
        .ok_or("all hits empty after length check")?;
    if first_all.candidate_id != "c3" {
        return Err(format!(
            "expected id c3 for All([fox, quick]), got {}",
            first_all.candidate_id
        )
        .into());
    }

    // 4) Regex leaf executes directly against the indexed content field.
    let regex_hits = searcher.search(
        &make_query(LqExpr::Leaf(LqLeaf::Regex("qu.*k".to_string()))),
        10,
    )?;
    if regex_hits.len() != 1 {
        return Err(format!(
            "expected 1 candidate for regex `qu.*k`, got {}",
            regex_hits.len()
        )
        .into());
    }
    let first_regex = regex_hits
        .first()
        .ok_or("regex hits empty after length check")?;
    if first_regex.candidate_id != "c3" {
        return Err(format!(
            "expected id c3 for regex `qu.*k`, got {}",
            first_regex.candidate_id
        )
        .into());
    }

    let mut regexp_option_query = make_query(LqExpr::Leaf(LqLeaf::Keyword("qu.*k".to_string())));
    regexp_option_query.options.pattern_type = LqPatternType::Regexp;
    let regexp_option_hits = searcher.search(&regexp_option_query, 10)?;
    if regexp_option_hits.len() != 1 {
        return Err(format!(
            "expected 1 candidate for patterntype:regexp `qu.*k`, got {}",
            regexp_option_hits.len()
        )
        .into());
    }
    let first_regexp_option = regexp_option_hits
        .first()
        .ok_or("regexp option hits empty after length check")?;
    if first_regexp_option.candidate_id != "c3" {
        return Err(format!(
            "expected id c3 for patterntype:regexp `qu.*k`, got {}",
            first_regexp_option.candidate_id
        )
        .into());
    }

    let raw_hits = searcher.search(
        &make_query(LqExpr::Leaf(LqLeaf::RawString("x ju".to_string()))),
        10,
    )?;
    if raw_hits.len() != 1 {
        return Err(format!(
            "expected 1 candidate for raw substring `x ju`, got {}",
            raw_hits.len()
        )
        .into());
    }
    let first_raw = raw_hits
        .first()
        .ok_or("raw hits empty after length check")?;
    if first_raw.candidate_id != "c1" {
        return Err(format!(
            "expected id c1 for raw substring `x ju`, got {}",
            first_raw.candidate_id
        )
        .into());
    }

    Ok(())
}

#[test]
fn tantivy_executes_repo_file_path_and_lang_filters() -> TestResult {
    let dir = tempfile::tempdir()?;
    let adapter = LexicalAdapter::with_state_root(dir.path().to_path_buf());

    let ops = vec![
        upsert_with_metadata("alpha", "src/lib.rs", "rust", 4, 8, "needle alpha")?,
        upsert_with_metadata("beta", "src/main.rs", "rust", 10, 12, "needle beta")?,
        upsert_with_metadata("gamma", "src/lib.py", "python", 20, 24, "needle gamma")?,
    ];
    adapter.build(&repo(), &revision(), generation(), &ops)?;

    let searcher = adapter.open(&repo(), &revision(), generation())?;

    let exact_path_hits = searcher.search(
        &make_query_with_filters(
            LqExpr::Leaf(LqLeaf::Keyword("needle".to_string())),
            vec![
                LqFilter::Repo {
                    pattern: repo().as_str().to_string(),
                    revs: Vec::new(),
                },
                LqFilter::File {
                    pattern: "src/lib.rs".to_string(),
                    scope: LqFileScope::PathOnly,
                },
                LqFilter::Lang {
                    id: "rust".to_string(),
                },
            ],
        ),
        10,
    )?;
    if exact_path_hits.len() != 1 {
        return Err(format!(
            "expected 1 exact path/lang hit, got {}",
            exact_path_hits.len()
        )
        .into());
    }
    let exact_path = exact_path_hits
        .first()
        .ok_or("exact path hits empty after length check")?;
    if exact_path.candidate_id != "alpha" {
        return Err(format!(
            "expected alpha for exact path/lang, got {}",
            exact_path.candidate_id
        )
        .into());
    }

    let file_hits = searcher.search(
        &make_query_with_filters(
            LqExpr::Leaf(LqLeaf::Keyword("needle".to_string())),
            vec![LqFilter::File {
                pattern: "main.rs".to_string(),
                scope: LqFileScope::NameAndPath,
            }],
        ),
        10,
    )?;
    if file_hits.len() != 1 {
        return Err(format!("expected 1 file-scope hit, got {}", file_hits.len()).into());
    }
    let file_hit = file_hits
        .first()
        .ok_or("file hits empty after length check")?;
    if file_hit.candidate_id != "beta" {
        return Err(format!(
            "expected beta for file-scope filter, got {}",
            file_hit.candidate_id
        )
        .into());
    }

    let repo_miss = searcher.search(
        &make_query_with_filters(
            LqExpr::Leaf(LqLeaf::Keyword("needle".to_string())),
            vec![LqFilter::Repo {
                pattern: "other-repo".to_string(),
                revs: Vec::new(),
            }],
        ),
        10,
    )?;
    if !repo_miss.is_empty() {
        return Err(format!("expected repo mismatch to return 0 hits, got {repo_miss:?}").into());
    }

    Ok(())
}

#[test]
fn tantivy_executes_phrase_adjacency_without_unordered_match() -> TestResult {
    let dir = tempfile::tempdir()?;
    let adapter = LexicalAdapter::with_state_root(dir.path().to_path_buf());

    let ops = vec![
        upsert("alpha", "the lemon yellow banana ripens")?,
        upsert("beta", "banana near lemon but not adjacent")?,
    ];
    adapter.build(&repo(), &revision(), generation(), &ops)?;

    let searcher = adapter.open(&repo(), &revision(), generation())?;

    let exact_phrase_hits = searcher.search(
        &make_query(LqExpr::Leaf(LqLeaf::Phrase(
            "lemon yellow banana".to_string(),
        ))),
        10,
    )?;
    if exact_phrase_hits.len() != 1 {
        return Err(format!(
            "expected 1 exact phrase hit, got {}",
            exact_phrase_hits.len()
        )
        .into());
    }
    let exact_phrase = exact_phrase_hits
        .first()
        .ok_or("exact phrase hits empty after length check")?;
    if exact_phrase.candidate_id != "alpha" {
        return Err(format!(
            "expected alpha for exact phrase, got {}",
            exact_phrase.candidate_id
        )
        .into());
    }

    let reversed_phrase_hits = searcher.search(
        &make_query(LqExpr::Leaf(LqLeaf::Phrase("banana lemon".to_string()))),
        10,
    )?;
    if !reversed_phrase_hits.is_empty() {
        return Err(
            format!("expected 0 hits for reversed phrase, got {reversed_phrase_hits:?}").into(),
        );
    }

    Ok(())
}

#[test]
fn tantivy_phrase_sidecar_uses_text_authority_and_case_rules() -> TestResult {
    let dir = tempfile::tempdir()?;
    let adapter = LexicalAdapter::with_state_root(dir.path().to_path_buf());

    let ops = vec![
        upsert_with_texts(
            "alpha",
            "src/lib.rs",
            "rust",
            4,
            8,
            "Lemon Yellow Banana",
            "Lemon Yellow Banana",
        )?,
        upsert_with_texts(
            "beta",
            "src/lib.rs",
            "rust",
            10,
            12,
            "banana near lemon but not adjacent",
            "banana near lemon but not adjacent",
        )?,
    ];
    adapter.build(&repo(), &revision(), generation(), &ops)?;

    let searcher = adapter.open(&repo(), &revision(), generation())?;

    let exact_hits = searcher.search(
        &make_query(LqExpr::Leaf(LqLeaf::Phrase(
            "lemon yellow banana".to_string(),
        ))),
        10,
    )?;
    let Some(exact_hit) = exact_hits.first() else {
        return Err("expected text-backed phrase hit [alpha], got []".into());
    };
    if exact_hits.len() != 1 || exact_hit.candidate_id != "alpha" {
        return Err(format!("expected text-backed phrase hit [alpha], got {exact_hits:?}").into());
    }
    if exact_hit.snippet != "Lemon Yellow Banana" {
        return Err(format!(
            "expected returned snippet to be derived from text authority, got {:?}",
            exact_hit.snippet
        )
        .into());
    }

    let mut sensitive_miss = make_query(LqExpr::Leaf(LqLeaf::Phrase(
        "lemon yellow banana".to_string(),
    )));
    sensitive_miss.options.case = Some(LqCase::Sensitive);
    let sensitive_miss_hits = searcher.search(&sensitive_miss, 10)?;
    if !sensitive_miss_hits.is_empty() {
        return Err(format!(
            "expected case:yes lowercase phrase to miss mixed-case text authority, got {sensitive_miss_hits:?}"
        )
        .into());
    }

    let mut sensitive_hit = make_query(LqExpr::Leaf(LqLeaf::Phrase(
        "Lemon Yellow Banana".to_string(),
    )));
    sensitive_hit.options.case = Some(LqCase::Sensitive);
    let sensitive_hit_hits = searcher.search(&sensitive_hit, 10)?;
    let Some(sensitive_hit) = sensitive_hit_hits.first() else {
        return Err("expected case:yes exact-case phrase hit [alpha], got []".into());
    };
    if sensitive_hit_hits.len() != 1 || sensitive_hit.candidate_id != "alpha" {
        return Err(format!(
            "expected case:yes exact-case phrase hit [alpha], got {sensitive_hit_hits:?}"
        )
        .into());
    }

    Ok(())
}

#[test]
fn tantivy_executes_whole_document_regex_and_rejects_false_positive() -> TestResult {
    let dir = tempfile::tempdir()?;
    let adapter = LexicalAdapter::with_state_root(dir.path().to_path_buf());

    let ops = vec![
        upsert("alpha", "const VERSION: &str = \"v1.2.3-rc.4\";")?,
        upsert("beta", "let needle_xx = 1;")?,
    ];
    adapter.build(&repo(), &revision(), generation(), &ops)?;

    let searcher = adapter.open(&repo(), &revision(), generation())?;

    let regex_hits = searcher.search(
        &make_query(LqExpr::Leaf(LqLeaf::Regex(
            "v\\d+\\.\\d+\\.\\d+".to_string(),
        ))),
        10,
    )?;
    if regex_hits.len() != 1 {
        return Err(format!(
            "expected 1 whole-document regex hit, got {}",
            regex_hits.len()
        )
        .into());
    }
    let regex_hit = regex_hits
        .first()
        .ok_or("regex hits empty after length check")?;
    if regex_hit.candidate_id != "alpha" {
        return Err(format!(
            "expected alpha for whole-document regex, got {}",
            regex_hit.candidate_id
        )
        .into());
    }

    let mut regexp_option_query = make_query(LqExpr::Leaf(LqLeaf::Keyword(
        "v\\d+\\.\\d+\\.\\d+".to_string(),
    )));
    regexp_option_query.options.pattern_type = LqPatternType::Regexp;
    let regexp_option_hits = searcher.search(&regexp_option_query, 10)?;
    if regexp_option_hits.len() != 1 {
        return Err(format!(
            "expected 1 patterntype:regexp hit, got {}",
            regexp_option_hits.len()
        )
        .into());
    }
    let regexp_option_hit = regexp_option_hits
        .first()
        .ok_or("regexp option hits empty after length check")?;
    if regexp_option_hit.candidate_id != "alpha" {
        return Err(format!(
            "expected alpha for patterntype:regexp, got {}",
            regexp_option_hit.candidate_id
        )
        .into());
    }

    let false_positive_hits = searcher.search(
        &make_query(LqExpr::Leaf(LqLeaf::Regex("needle_x[0-9]".to_string()))),
        10,
    )?;
    if !false_positive_hits.is_empty() {
        return Err(format!(
            "expected 0 hits for regex false-positive bait, got {false_positive_hits:?}"
        )
        .into());
    }

    Ok(())
}

#[test]
fn tantivy_regex_sidecar_verifies_authoritative_text() -> TestResult {
    let dir = tempfile::tempdir()?;
    let adapter = LexicalAdapter::with_state_root(dir.path().to_path_buf());

    let ops = vec![
        upsert_with_texts(
            "alpha",
            "src/lib.rs",
            "rust",
            4,
            8,
            "const VERSION: &str = \"v1.2.3-rc.4\";",
            "const VERSION: &str = \"v1.2.3-rc.4\";",
        )?,
        upsert_with_texts(
            "beta",
            "src/lib.rs",
            "rust",
            10,
            12,
            "let needle_xx = 1;",
            "let needle_xx = 1;",
        )?,
    ];
    adapter.build(&repo(), &revision(), generation(), &ops)?;

    let searcher = adapter.open(&repo(), &revision(), generation())?;

    let regex_hits = searcher.search(
        &make_query(LqExpr::Leaf(LqLeaf::Regex(
            "v\\d+\\.\\d+\\.\\d+".to_string(),
        ))),
        10,
    )?;
    let Some(regex_hit) = regex_hits.first() else {
        return Err("expected authoritative text regex hit [alpha], got []".into());
    };
    if regex_hits.len() != 1 || regex_hit.candidate_id != "alpha" {
        return Err(
            format!("expected authoritative text regex hit [alpha], got {regex_hits:?}").into(),
        );
    }
    if regex_hit.snippet != "const VERSION: &str = \"v1.2.3-rc.4\";" {
        return Err(format!(
            "expected returned snippet to be derived from text authority, got {:?}",
            regex_hit.snippet
        )
        .into());
    }

    let false_positive_hits = searcher.search(
        &make_query(LqExpr::Leaf(LqLeaf::Regex("needle_x[0-9]".to_string()))),
        10,
    )?;
    if !false_positive_hits.is_empty() {
        return Err(format!(
            "expected regex exact verify to reject text-authority false positive bait, got {false_positive_hits:?}"
        )
        .into());
    }

    Ok(())
}

#[test]
fn tantivy_executes_supported_type_and_select_filters() -> TestResult {
    let dir = tempfile::tempdir()?;
    let adapter = LexicalAdapter::with_state_root(dir.path().to_path_buf());

    let ops = vec![
        upsert_with_metadata(
            "alpha",
            "src/lib.rs",
            "rust",
            4,
            8,
            "needle alpha projection projection",
        )?,
        upsert_with_metadata(
            "beta",
            "src/main.rs",
            "rust",
            10,
            12,
            "needle beta projection",
        )?,
        upsert_symbol("sym-alpha", "src/lib.rs", "rust", "needle_symbol", 4, 4)?,
    ];
    adapter.build(&repo(), &revision(), generation(), &ops)?;

    let searcher = adapter.open(&repo(), &revision(), generation())?;

    let type_symbol_hits = searcher.search(
        &make_query_with_filters(
            LqExpr::Leaf(LqLeaf::Keyword("needle_symbol".to_string())),
            vec![LqFilter::Type {
                kind: LqType::Symbol,
            }],
        ),
        10,
    )?;
    if type_symbol_hits.len() != 1 {
        return Err(format!("expected 1 type:symbol hit, got {}", type_symbol_hits.len()).into());
    }
    let first_type_symbol = type_symbol_hits
        .first()
        .ok_or("type:symbol hits empty after length check")?;
    if first_type_symbol.candidate_id != "sym-alpha" {
        return Err(format!(
            "expected sym-alpha for type:symbol, got {}",
            first_type_symbol.candidate_id
        )
        .into());
    }

    let select_symbol_hits = searcher.search(
        &make_query_with_filters(
            LqExpr::Leaf(LqLeaf::Keyword("needle_symbol".to_string())),
            vec![LqFilter::Select {
                dim: LqSelect::Symbol,
            }],
        ),
        10,
    )?;
    if select_symbol_hits.len() != 1 {
        return Err(format!(
            "expected 1 select:symbol hit, got {}",
            select_symbol_hits.len()
        )
        .into());
    }
    let first_select_symbol = select_symbol_hits
        .first()
        .ok_or("select:symbol hits empty after length check")?;
    if first_select_symbol.candidate_id != "sym-alpha" {
        return Err(format!(
            "expected sym-alpha for select:symbol, got {}",
            first_select_symbol.candidate_id
        )
        .into());
    }

    let symbol_predicate_hits = searcher.search(
        &make_query(LqExpr::Leaf(LqLeaf::Predicate {
            name: "symbol.has.name".to_string(),
            args: vec![LqPredicateArg::Keyword("needle_symbol".to_string())],
        })),
        10,
    )?;
    if symbol_predicate_hits.len() != 1 {
        return Err(format!(
            "expected 1 symbol.has.name hit, got {}",
            symbol_predicate_hits.len()
        )
        .into());
    }
    let first_symbol_predicate = symbol_predicate_hits
        .first()
        .ok_or("symbol.has.name hits empty after length check")?;
    if first_symbol_predicate.candidate_id != "sym-alpha" {
        return Err(format!(
            "expected sym-alpha for symbol.has.name, got {}",
            first_symbol_predicate.candidate_id
        )
        .into());
    }

    let text_hits = searcher.search(
        &make_query_with_filters(
            LqExpr::Leaf(LqLeaf::Keyword("alpha".to_string())),
            vec![LqFilter::Type { kind: LqType::File }],
        ),
        10,
    )?;
    let first_text_hit = text_hits
        .first()
        .ok_or("type:file hits empty after length check")?;
    if text_hits.len() != 1 || first_text_hit.candidate_id != "alpha" {
        return Err(format!("expected alpha for type:file, got {text_hits:?}").into());
    }

    let select_repo_hits = searcher.search(
        &make_query_with_filters(
            LqExpr::Leaf(LqLeaf::Keyword("projection".to_string())),
            vec![LqFilter::Select {
                dim: LqSelect::Repo,
            }],
        ),
        10,
    )?;
    if select_repo_hits.len() != 1 {
        return Err(format!(
            "expected 1 select:repo representative hit, got {}",
            select_repo_hits.len()
        )
        .into());
    }
    let first_select_repo = select_repo_hits
        .first()
        .ok_or("select:repo hits empty after length check")?;
    if first_select_repo.candidate_id != "alpha" {
        return Err(format!(
            "expected alpha as select:repo representative, got {}",
            first_select_repo.candidate_id
        )
        .into());
    }

    Ok(())
}

#[test]
fn tantivy_select_repo_uses_canonical_representative_when_scores_tie() -> TestResult {
    let dir = tempfile::tempdir()?;
    let adapter = LexicalAdapter::with_state_root(dir.path().to_path_buf());

    let ops = vec![
        upsert_with_metadata("zeta", "src/zeta.rs", "rust", 10, 12, "repo_tie_needle")?,
        upsert_with_metadata("alpha", "src/alpha.rs", "rust", 4, 8, "repo_tie_needle")?,
    ];
    adapter.build(&repo(), &revision(), generation(), &ops)?;

    let searcher = adapter.open(&repo(), &revision(), generation())?;
    let hits = searcher.search(
        &make_query_with_filters(
            LqExpr::Leaf(LqLeaf::Keyword("repo_tie_needle".to_string())),
            vec![LqFilter::Select {
                dim: LqSelect::Repo,
            }],
        ),
        10,
    )?;
    if hits.len() != 1 {
        return Err(format!(
            "expected 1 select:repo representative under tie, got {}",
            hits.len()
        )
        .into());
    }
    let first = hits
        .first()
        .ok_or("select:repo tie hits empty after length check")?;
    if first.candidate_id != "alpha" {
        return Err(format!(
            "expected canonical select:repo representative `alpha`, got {}",
            first.candidate_id
        )
        .into());
    }

    Ok(())
}

#[test]
fn tantivy_executes_simple_path_term_surface_without_boolean_path_leakage() -> TestResult {
    let dir = tempfile::tempdir()?;
    let adapter = LexicalAdapter::with_state_root(dir.path().to_path_buf());

    let ops = vec![
        upsert_with_metadata("alpha", "src/lib.rs", "rust", 4, 8, "needle alpha")?,
        upsert_with_metadata(
            "beta",
            "config/path_only_needle.toml",
            "toml",
            1,
            1,
            "value = 1",
        )?,
    ];
    adapter.build(&repo(), &revision(), generation(), &ops)?;

    let searcher = adapter.open(&repo(), &revision(), generation())?;

    let path_hits = searcher.search(
        &make_query(LqExpr::Leaf(LqLeaf::Keyword(
            "path_only_needle".to_string(),
        ))),
        10,
    )?;
    if path_hits.len() != 1 {
        return Err(format!("expected 1 simple path-term hit, got {}", path_hits.len()).into());
    }
    let first_path = path_hits
        .first()
        .ok_or("path hits empty after length check")?;
    if first_path.candidate_id != "beta" {
        return Err(format!(
            "expected beta for simple path-term surface, got {}",
            first_path.candidate_id
        )
        .into());
    }

    let boolean_hits = searcher.search(
        &make_query(LqExpr::All(vec![
            LqExpr::Leaf(LqLeaf::Keyword("alpha".to_string())),
            LqExpr::Not(Box::new(LqExpr::Leaf(LqLeaf::Keyword("lib".to_string())))),
        ])),
        10,
    )?;
    if boolean_hits.len() != 1 {
        return Err(format!(
            "expected boolean NOT to stay content-only, got {} hits",
            boolean_hits.len()
        )
        .into());
    }
    let first_boolean = boolean_hits
        .first()
        .ok_or("boolean hits empty after length check")?;
    if first_boolean.candidate_id != "alpha" {
        return Err(format!(
            "expected alpha to survive boolean NOT path leakage guard, got {}",
            first_boolean.candidate_id
        )
        .into());
    }

    Ok(())
}

#[test]
fn tantivy_honors_case_sensitive_keyword_queries() -> TestResult {
    let dir = tempfile::tempdir()?;
    let adapter = LexicalAdapter::with_state_root(dir.path().to_path_buf());

    let ops = vec![upsert_with_metadata(
        "alpha",
        "src/lib.rs",
        "rust",
        4,
        8,
        "fn alpha_content_needle() {}",
    )?];
    adapter.build(&repo(), &revision(), generation(), &ops)?;

    let searcher = adapter.open(&repo(), &revision(), generation())?;

    let default_hits = searcher.search(
        &make_query(LqExpr::Leaf(LqLeaf::Keyword(
            "ALPHA_CONTENT_NEEDLE".to_string(),
        ))),
        10,
    )?;
    if default_hits.len() != 1 {
        return Err(format!(
            "expected default case-insensitive query to hit once, got {}",
            default_hits.len()
        )
        .into());
    }

    let mut sensitive_query = make_query(LqExpr::Leaf(LqLeaf::Keyword(
        "ALPHA_CONTENT_NEEDLE".to_string(),
    )));
    sensitive_query.options.case = Some(LqCase::Sensitive);
    let sensitive_hits = searcher.search(&sensitive_query, 10)?;
    if !sensitive_hits.is_empty() {
        return Err(format!(
            "expected case:yes uppercase query to miss lowercase content, got {sensitive_hits:?}"
        )
        .into());
    }

    let mut exact_case_query = make_query(LqExpr::Leaf(LqLeaf::Keyword(
        "alpha_content_needle".to_string(),
    )));
    exact_case_query.options.case = Some(LqCase::Sensitive);
    let exact_case_hits = searcher.search(&exact_case_query, 10)?;
    if exact_case_hits.len() != 1 {
        return Err(format!(
            "expected case:yes exact-case query to hit once, got {}",
            exact_case_hits.len()
        )
        .into());
    }

    Ok(())
}

#[test]
fn tantivy_top_k_stabilizes_keyword_path_surface_without_losing_content_hits() -> TestResult {
    let dir = tempfile::tempdir()?;
    let adapter = LexicalAdapter::with_state_root(dir.path().to_path_buf());

    let ops = vec![
        upsert_with_metadata(
            "alpha",
            "src/lib.rs",
            "rust",
            1,
            1,
            "fn alpha_content_needle() {}",
        )?,
        upsert_with_metadata(
            "beta",
            "config/path_only_needle.toml",
            "toml",
            1,
            1,
            "value = 1",
        )?,
        upsert_with_metadata(
            "gamma",
            "scripts/helper.py",
            "python",
            1,
            1,
            "def alpha_content_needle(): pass",
        )?,
        upsert_with_metadata(
            "delta",
            "docs/colors.md",
            "markdown",
            1,
            1,
            "the lemon yellow banana ripens",
        )?,
        upsert_with_metadata(
            "epsilon",
            "src/version.rs",
            "rust",
            1,
            1,
            "const VERSION: &str = \"v1.2.3-rc.4\";",
        )?,
        upsert_with_metadata("zeta", "src/raw.rs", "rust", 1, 1, "let foo_bar_baz = 0;")?,
        upsert_with_metadata("eta", "src/bait.rs", "rust", 1, 1, "let needle_xx = 1;")?,
        upsert_symbol("theta", "src/sym.rs", "rust", "MyTypeSymbol", 1, 1)?,
    ];
    adapter.build(&repo(), &revision(), generation(), &ops)?;

    let searcher = adapter.open(&repo(), &revision(), generation())?;
    let mut query = make_query(LqExpr::Leaf(LqLeaf::Keyword("needle".to_string())));
    query.options.count = Some(LqCountBound::Bounded(2));
    let hits = searcher.search(&query, 10)?;
    let ids = hits
        .iter()
        .map(|hit| hit.candidate_id.as_str())
        .collect::<Vec<_>>();
    if ids != ["beta", "eta"] {
        return Err(format!("expected top_k ids [beta, eta], got hits {hits:?}").into());
    }

    Ok(())
}

#[test]
fn tantivy_count_all_ignores_request_top_k_and_returns_full_recall() -> TestResult {
    let dir = tempfile::tempdir()?;
    let adapter = LexicalAdapter::with_state_root(dir.path().to_path_buf());

    let ops = vec![
        upsert_with_metadata("alpha", "src/lib.rs", "rust", 4, 8, "count_all_needle")?,
        upsert_with_metadata("beta", "src/lib.rs", "rust", 20, 24, "count_all_needle")?,
        upsert_with_metadata("gamma", "src/main.rs", "rust", 10, 12, "count_all_needle")?,
    ];
    adapter.build(&repo(), &revision(), generation(), &ops)?;

    let searcher = adapter.open(&repo(), &revision(), generation())?;
    let mut query = make_query(LqExpr::Leaf(LqLeaf::Keyword(
        "count_all_needle".to_string(),
    )));
    query.options.count = Some(LqCountBound::All);
    let hits = searcher.search(&query, 1)?;
    let ids = hits
        .iter()
        .map(|hit| hit.candidate_id.as_str())
        .collect::<Vec<_>>();
    if ids != ["alpha", "beta", "gamma"] {
        return Err(format!(
            "expected count:all full recall ids [alpha, beta, gamma], got hits {hits:?}"
        )
        .into());
    }

    Ok(())
}

#[test]
fn tantivy_select_file_collapses_multiple_chunks_per_path() -> TestResult {
    let dir = tempfile::tempdir()?;
    let adapter = LexicalAdapter::with_state_root(dir.path().to_path_buf());

    let ops = vec![
        upsert_with_metadata(
            "alpha",
            "src/lib.rs",
            "rust",
            4,
            8,
            "file_projection_needle",
        )?,
        upsert_with_metadata(
            "beta",
            "src/lib.rs",
            "rust",
            20,
            24,
            "file_projection_needle",
        )?,
        upsert_with_metadata(
            "gamma",
            "src/main.rs",
            "rust",
            10,
            12,
            "file_projection_needle",
        )?,
    ];
    adapter.build(&repo(), &revision(), generation(), &ops)?;

    let searcher = adapter.open(&repo(), &revision(), generation())?;
    let hits = searcher.search(
        &make_query_with_filters(
            LqExpr::Leaf(LqLeaf::Keyword("file_projection_needle".to_string())),
            vec![LqFilter::Select {
                dim: LqSelect::File,
            }],
        ),
        10,
    )?;
    let hit_ids: Vec<&str> = hits.iter().map(|hit| hit.candidate_id.as_str()).collect();
    if hit_ids != vec!["alpha", "gamma"] {
        return Err(format!(
            "expected select:file to collapse to per-path representatives [alpha, gamma], got {hit_ids:?}"
        )
        .into());
    }

    Ok(())
}

#[test]
fn tantivy_select_path_collapses_multiple_chunks_per_path() -> TestResult {
    let dir = tempfile::tempdir()?;
    let adapter = LexicalAdapter::with_state_root(dir.path().to_path_buf());

    let ops = vec![
        upsert_with_metadata(
            "alpha",
            "src/lib.rs",
            "rust",
            4,
            8,
            "path_projection_needle",
        )?,
        upsert_with_metadata(
            "beta",
            "src/lib.rs",
            "rust",
            20,
            24,
            "path_projection_needle",
        )?,
        upsert_with_metadata(
            "gamma",
            "src/main.rs",
            "rust",
            10,
            12,
            "path_projection_needle",
        )?,
    ];
    adapter.build(&repo(), &revision(), generation(), &ops)?;

    let searcher = adapter.open(&repo(), &revision(), generation())?;
    let hits = searcher.search(
        &make_query_with_filters(
            LqExpr::Leaf(LqLeaf::Keyword("path_projection_needle".to_string())),
            vec![LqFilter::Select {
                dim: LqSelect::Path,
            }],
        ),
        10,
    )?;
    let hit_ids: Vec<&str> = hits.iter().map(|hit| hit.candidate_id.as_str()).collect();
    if hit_ids != vec!["alpha", "gamma"] {
        return Err(format!(
            "expected select:path to collapse to per-path representatives [alpha, gamma], got {hit_ids:?}"
        )
        .into());
    }

    Ok(())
}

#[test]
fn tantivy_type_path_collapses_multiple_chunks_per_path() -> TestResult {
    let dir = tempfile::tempdir()?;
    let adapter = LexicalAdapter::with_state_root(dir.path().to_path_buf());

    let ops = vec![
        upsert_with_metadata(
            "alpha",
            "src/lib.rs",
            "rust",
            4,
            8,
            "type_path_projection_needle",
        )?,
        upsert_with_metadata(
            "beta",
            "src/lib.rs",
            "rust",
            20,
            24,
            "type_path_projection_needle",
        )?,
        upsert_with_metadata(
            "gamma",
            "src/main.rs",
            "rust",
            10,
            12,
            "type_path_projection_needle",
        )?,
    ];
    adapter.build(&repo(), &revision(), generation(), &ops)?;

    let searcher = adapter.open(&repo(), &revision(), generation())?;
    let hits = searcher.search(
        &make_query_with_filters(
            LqExpr::Leaf(LqLeaf::Keyword("type_path_projection_needle".to_string())),
            vec![LqFilter::Type { kind: LqType::Path }],
        ),
        10,
    )?;
    let hit_ids: Vec<&str> = hits.iter().map(|hit| hit.candidate_id.as_str()).collect();
    if hit_ids != vec!["alpha", "gamma"] {
        return Err(format!(
            "expected type:path to collapse to per-path representatives [alpha, gamma], got {hit_ids:?}"
        )
        .into());
    }

    Ok(())
}

#[test]
fn tantivy_type_repo_collapses_to_repo_representative() -> TestResult {
    let dir = tempfile::tempdir()?;
    let adapter = LexicalAdapter::with_state_root(dir.path().to_path_buf());

    let ops = vec![
        upsert_with_metadata("zeta", "src/zeta.rs", "rust", 10, 12, "type_repo_needle")?,
        upsert_with_metadata("alpha", "src/alpha.rs", "rust", 4, 8, "type_repo_needle")?,
    ];
    adapter.build(&repo(), &revision(), generation(), &ops)?;

    let searcher = adapter.open(&repo(), &revision(), generation())?;
    let hits = searcher.search(
        &make_query_with_filters(
            LqExpr::Leaf(LqLeaf::Keyword("type_repo_needle".to_string())),
            vec![LqFilter::Type { kind: LqType::Repo }],
        ),
        10,
    )?;
    if hits.len() != 1 {
        return Err(format!(
            "expected 1 type:repo representative under tie, got {}",
            hits.len()
        )
        .into());
    }
    let first = hits
        .first()
        .ok_or("type:repo tie hits empty after length check")?;
    if first.candidate_id != "alpha" {
        return Err(format!(
            "expected canonical type:repo representative `alpha`, got {}",
            first.candidate_id
        )
        .into());
    }

    Ok(())
}

#[test]
fn tantivy_executes_repo_metadata_filters_when_bundle_payload_is_typed() -> TestResult {
    let dir = tempfile::tempdir()?;
    let adapter = LexicalAdapter::with_state_root(dir.path().to_path_buf());

    let ops = vec![
        LexicalChannelOp::FullBundle(LexicalFullBundle {
            repo_id: repo(),
            revision_id: revision(),
            generation: generation(),
            payload: encode_repo_metadata_payload(
                true,
                false,
                LqVisibility::Private,
                &["team-search", "staging"],
            )?,
        }),
        upsert_with_metadata("alpha", "src/lib.rs", "rust", 4, 8, "needle alpha")?,
        upsert_with_metadata("beta", "src/main.rs", "rust", 10, 12, "needle beta")?,
    ];
    adapter.build(&repo(), &revision(), generation(), &ops)?;

    let searcher = adapter.open(&repo(), &revision(), generation())?;
    let matching_hits = searcher.search(
        &make_query_with_filters(
            LqExpr::Leaf(LqLeaf::Keyword("needle".to_string())),
            vec![
                LqFilter::Fork {
                    mode: LqYesNoOnly::Only,
                },
                LqFilter::Archived {
                    mode: LqYesNoOnly::No,
                },
                LqFilter::Visibility {
                    mode: LqVisibility::Private,
                },
                LqFilter::Context {
                    name: "team-search".to_string(),
                },
            ],
        ),
        10,
    )?;
    if matching_hits.len() != 2 {
        return Err(format!(
            "expected 2 metadata-filtered hits, got {}",
            matching_hits.len()
        )
        .into());
    }

    let reopened_adapter = LexicalAdapter::with_state_root(dir.path().to_path_buf());
    let reopened_searcher = reopened_adapter.open(&repo(), &revision(), generation())?;
    let reopened_hits = reopened_searcher.search(
        &make_query_with_filters(
            LqExpr::Leaf(LqLeaf::Keyword("needle".to_string())),
            vec![LqFilter::Context {
                name: "team-search".to_string(),
            }],
        ),
        10,
    )?;
    if reopened_hits.len() != 2 {
        return Err(format!(
            "expected reopened adapter to reload 2 metadata-filtered hits, got {}",
            reopened_hits.len()
        )
        .into());
    }

    let mismatched_hits = searcher.search(
        &make_query_with_filters(
            LqExpr::Leaf(LqLeaf::Keyword("needle".to_string())),
            vec![LqFilter::Context {
                name: "prod".to_string(),
            }],
        ),
        10,
    )?;
    if !mismatched_hits.is_empty() {
        return Err(
            format!("expected context mismatch to return 0 hits, got {mismatched_hits:?}").into(),
        );
    }

    Ok(())
}

#[test]
fn tantivy_executes_repo_allow_list_across_indexed_source_repo_ids() -> TestResult {
    let dir = tempfile::tempdir()?;
    let adapter = LexicalAdapter::with_state_root(dir.path().to_path_buf());

    let ops = vec![
        upsert_with_source_repo(
            "corp-a-doc",
            "src/corp-a.rs",
            "rust",
            1,
            2,
            "shared_oracle_needle corp-a branch",
            "corp-a",
        )?,
        upsert_with_source_repo(
            "corp-b-doc",
            "src/corp-b.rs",
            "rust",
            1,
            2,
            "shared_oracle_needle corp-b branch",
            "corp-b",
        )?,
        upsert_with_source_repo(
            "corp-a-gate",
            "src/gate-a.rs",
            "rust",
            1,
            2,
            "shared_oracle_needle gate-a only 123",
            "corp-a",
        )?,
        upsert_with_source_repo(
            "corp-b-name-only",
            "lib/gate-a.rs",
            "rust",
            1,
            2,
            "shared_oracle_needle corp-b gate-a branch",
            "corp-b",
        )?,
        upsert_with_source_repo(
            "corp-b-path-lang",
            "src/gate-b.py",
            "python",
            1,
            2,
            "shared_oracle_needle corp-b python branch",
            "corp-b",
        )?,
    ];
    adapter.build(&repo(), &revision(), generation(), &ops)?;

    let searcher = adapter.open(&repo(), &revision(), generation())?;

    let corp_a_hits = searcher.search(
        &make_query_with_filters(
            LqExpr::Leaf(LqLeaf::Keyword("shared_oracle_needle".to_string())),
            vec![LqFilter::Repo {
                pattern: "corp-a".to_string(),
                revs: Vec::new(),
            }],
        ),
        10,
    )?;
    let mut corp_a_ids: Vec<String> = corp_a_hits
        .into_iter()
        .map(|hit| hit.candidate_id)
        .collect();
    corp_a_ids.sort();
    if corp_a_ids != vec!["corp-a-doc".to_string(), "corp-a-gate".to_string()] {
        return Err(format!("expected corp-a allow-list ids, got {corp_a_ids:?}").into());
    }

    let corp_b_hits = searcher.search(
        &make_query_with_filters(
            LqExpr::Leaf(LqLeaf::Keyword("shared_oracle_needle".to_string())),
            vec![LqFilter::Repo {
                pattern: "corp-b".to_string(),
                revs: Vec::new(),
            }],
        ),
        10,
    )?;
    let mut corp_b_ids: Vec<String> = corp_b_hits
        .into_iter()
        .map(|hit| hit.candidate_id)
        .collect();
    corp_b_ids.sort();
    if corp_b_ids
        != vec![
            "corp-b-doc".to_string(),
            "corp-b-name-only".to_string(),
            "corp-b-path-lang".to_string(),
        ]
    {
        return Err(format!("expected corp-b allow-list ids, got {corp_b_ids:?}").into());
    }

    let universe_hits = searcher.search(
        &make_query(LqExpr::Leaf(LqLeaf::Keyword(
            "shared_oracle_needle".to_string(),
        ))),
        10,
    )?;
    if universe_hits.len() != 5 {
        return Err(format!(
            "expected 5 universe hits across indexed source repos, got {}",
            universe_hits.len()
        )
        .into());
    }

    Ok(())
}

#[test]
fn tantivy_repo_has_file_true_gate_narrows_by_indexed_source_repo_id() -> TestResult {
    let dir = tempfile::tempdir()?;
    let adapter = LexicalAdapter::with_state_root(dir.path().to_path_buf());

    let ops = vec![
        upsert_with_source_repo(
            "corp-a-doc",
            "src/corp-a.rs",
            "rust",
            1,
            2,
            "shared_oracle_needle corp-a branch",
            "corp-a",
        )?,
        upsert_with_source_repo(
            "corp-b-doc",
            "src/corp-b.rs",
            "rust",
            1,
            2,
            "shared_oracle_needle corp-b branch",
            "corp-b",
        )?,
        upsert_with_source_repo(
            "corp-a-gate",
            "src/gate-a.rs",
            "rust",
            1,
            2,
            "shared_oracle_needle gate-a only 123",
            "corp-a",
        )?,
    ];
    adapter.build(&repo(), &revision(), generation(), &ops)?;

    let searcher = adapter.open(&repo(), &revision(), generation())?;
    let gate_query = make_query(LqExpr::All(vec![
        LqExpr::Leaf(LqLeaf::Predicate {
            name: "repo.has.file".to_string(),
            args: vec![LqPredicateArg::Filter {
                name: "path".to_string(),
                value: "src/gate-a.rs".to_string(),
            }],
        }),
        LqExpr::Leaf(LqLeaf::Keyword("shared_oracle_needle".to_string())),
    ]));
    let mut gate_ids: Vec<String> = searcher
        .search(&gate_query, 10)?
        .into_iter()
        .map(|hit| hit.candidate_id)
        .collect();
    gate_ids.sort();
    if gate_ids != vec!["corp-a-doc".to_string(), "corp-a-gate".to_string()] {
        return Err(
            format!("expected corp-a repo.has.file true-gate ids, got {gate_ids:?}").into(),
        );
    }

    let scalar_gate_query = make_query(LqExpr::All(vec![
        LqExpr::Leaf(LqLeaf::Predicate {
            name: "repo.has.file".to_string(),
            args: vec![LqPredicateArg::Keyword("src/gate-a.rs".to_string())],
        }),
        LqExpr::Leaf(LqLeaf::Keyword("shared_oracle_needle".to_string())),
    ]));
    let mut scalar_gate_ids: Vec<String> = searcher
        .search(&scalar_gate_query, 10)?
        .into_iter()
        .map(|hit| hit.candidate_id)
        .collect();
    scalar_gate_ids.sort();
    if scalar_gate_ids != vec!["corp-a-doc".to_string(), "corp-a-gate".to_string()] {
        return Err(format!(
            "expected corp-a repo.has.file(<scalar-path>) true-gate ids, got {scalar_gate_ids:?}"
        )
        .into());
    }

    let name_gate_query = make_query(LqExpr::All(vec![
        LqExpr::Leaf(LqLeaf::Predicate {
            name: "repo.has.file".to_string(),
            args: vec![LqPredicateArg::Filter {
                name: "name".to_string(),
                value: "gate-a.rs".to_string(),
            }],
        }),
        LqExpr::Leaf(LqLeaf::Keyword("shared_oracle_needle".to_string())),
    ]));
    let mut name_gate_ids: Vec<String> = searcher
        .search(&name_gate_query, 10)?
        .into_iter()
        .map(|hit| hit.candidate_id)
        .collect();
    name_gate_ids.sort();
    if name_gate_ids != vec!["corp-a-doc".to_string(), "corp-a-gate".to_string()] {
        return Err(format!(
            "expected corp-a repo.has.file(name:) true-gate ids, got {name_gate_ids:?}"
        )
        .into());
    }

    let name_miss_query = make_query(LqExpr::All(vec![
        LqExpr::Leaf(LqLeaf::Predicate {
            name: "repo.has.file".to_string(),
            args: vec![LqPredicateArg::Filter {
                name: "name".to_string(),
                value: "missing.rs".to_string(),
            }],
        }),
        LqExpr::Leaf(LqLeaf::Keyword("shared_oracle_needle".to_string())),
    ]));
    let name_miss_hits = searcher.search(&name_miss_query, 10)?;
    if !name_miss_hits.is_empty() {
        return Err(format!(
            "expected repo.has.file(name:) miss to return zero hits, got {name_miss_hits:?}"
        )
        .into());
    }

    let scalar_miss_query = make_query(LqExpr::All(vec![
        LqExpr::Leaf(LqLeaf::Predicate {
            name: "repo.has.file".to_string(),
            args: vec![LqPredicateArg::RawString("src/missing.rs".to_string())],
        }),
        LqExpr::Leaf(LqLeaf::Keyword("shared_oracle_needle".to_string())),
    ]));
    let scalar_miss_hits = searcher.search(&scalar_miss_query, 10)?;
    if !scalar_miss_hits.is_empty() {
        return Err(format!(
            "expected repo.has.file(<scalar-path>) miss to return zero hits, got {scalar_miss_hits:?}"
        )
        .into());
    }

    let combo_cases = [
        (
            "path+name",
            vec![
                LqPredicateArg::Filter {
                    name: "path".to_string(),
                    value: "src/gate-a.rs".to_string(),
                },
                LqPredicateArg::Filter {
                    name: "name".to_string(),
                    value: "gate-a.rs".to_string(),
                },
            ],
            vec!["corp-a-doc".to_string(), "corp-a-gate".to_string()],
        ),
        (
            "path+lang",
            vec![
                LqPredicateArg::Filter {
                    name: "path".to_string(),
                    value: "src/gate-a.rs".to_string(),
                },
                LqPredicateArg::Filter {
                    name: "lang".to_string(),
                    value: "rust".to_string(),
                },
            ],
            vec!["corp-a-doc".to_string(), "corp-a-gate".to_string()],
        ),
        (
            "name+lang",
            vec![
                LqPredicateArg::Filter {
                    name: "name".to_string(),
                    value: "gate-a.rs".to_string(),
                },
                LqPredicateArg::Filter {
                    name: "lang".to_string(),
                    value: "rust".to_string(),
                },
            ],
            vec!["corp-a-doc".to_string(), "corp-a-gate".to_string()],
        ),
        (
            "path+name+lang",
            vec![
                LqPredicateArg::Filter {
                    name: "path".to_string(),
                    value: "src/gate-a.rs".to_string(),
                },
                LqPredicateArg::Filter {
                    name: "name".to_string(),
                    value: "gate-a.rs".to_string(),
                },
                LqPredicateArg::Filter {
                    name: "lang".to_string(),
                    value: "rust".to_string(),
                },
            ],
            vec!["corp-a-doc".to_string(), "corp-a-gate".to_string()],
        ),
    ];
    for (label, args, expected) in combo_cases {
        let combo_query = make_query(LqExpr::All(vec![
            LqExpr::Leaf(LqLeaf::Predicate {
                name: "repo.has.file".to_string(),
                args,
            }),
            LqExpr::Leaf(LqLeaf::Keyword("shared_oracle_needle".to_string())),
        ]));
        let mut combo_ids: Vec<String> = searcher
            .search(&combo_query, 10)?
            .into_iter()
            .map(|hit| hit.candidate_id)
            .collect();
        combo_ids.sort();
        if combo_ids != expected {
            return Err(format!(
                "expected repo.has.file({label}) ids {expected:?}, got {combo_ids:?}"
            )
            .into());
        }
    }

    let combo_miss_cases = [
        (
            "path+name miss",
            vec![
                LqPredicateArg::Filter {
                    name: "path".to_string(),
                    value: "src/gate-a.rs".to_string(),
                },
                LqPredicateArg::Filter {
                    name: "name".to_string(),
                    value: "missing.rs".to_string(),
                },
            ],
        ),
        (
            "path+lang miss",
            vec![
                LqPredicateArg::Filter {
                    name: "path".to_string(),
                    value: "src/gate-a.rs".to_string(),
                },
                LqPredicateArg::Filter {
                    name: "lang".to_string(),
                    value: "go".to_string(),
                },
            ],
        ),
        (
            "name+lang miss",
            vec![
                LqPredicateArg::Filter {
                    name: "name".to_string(),
                    value: "gate-a.rs".to_string(),
                },
                LqPredicateArg::Filter {
                    name: "lang".to_string(),
                    value: "python".to_string(),
                },
            ],
        ),
        (
            "path+name+lang miss",
            vec![
                LqPredicateArg::Filter {
                    name: "path".to_string(),
                    value: "src/gate-a.rs".to_string(),
                },
                LqPredicateArg::Filter {
                    name: "name".to_string(),
                    value: "gate-a.rs".to_string(),
                },
                LqPredicateArg::Filter {
                    name: "lang".to_string(),
                    value: "python".to_string(),
                },
            ],
        ),
    ];
    for (label, args) in combo_miss_cases {
        let combo_query = make_query(LqExpr::All(vec![
            LqExpr::Leaf(LqLeaf::Predicate {
                name: "repo.has.file".to_string(),
                args,
            }),
            LqExpr::Leaf(LqLeaf::Keyword("shared_oracle_needle".to_string())),
        ]));
        let combo_hits = searcher.search(&combo_query, 10)?;
        if !combo_hits.is_empty() {
            return Err(
                format!("expected repo.has.file({label}) to miss, got {combo_hits:?}").into(),
            );
        }
    }

    Ok(())
}

#[test]
fn tantivy_repo_has_content_true_gate_narrows_by_indexed_source_repo_id() -> TestResult {
    let dir = tempfile::tempdir()?;
    let adapter = LexicalAdapter::with_state_root(dir.path().to_path_buf());

    let ops = vec![
        upsert_with_source_repo(
            "corp-a-doc",
            "src/corp-a.rs",
            "rust",
            1,
            2,
            "shared_oracle_needle corp-a branch",
            "corp-a",
        )?,
        upsert_with_source_repo(
            "corp-b-doc",
            "src/corp-b.rs",
            "rust",
            1,
            2,
            "shared_oracle_needle corp-b branch",
            "corp-b",
        )?,
        upsert_with_source_repo(
            "corp-a-gate",
            "src/gate-a.rs",
            "rust",
            1,
            2,
            "shared_oracle_needle gate-a only 123",
            "corp-a",
        )?,
    ];
    adapter.build(&repo(), &revision(), generation(), &ops)?;

    let searcher = adapter.open(&repo(), &revision(), generation())?;
    let gate_query = make_query(LqExpr::All(vec![
        LqExpr::Leaf(LqLeaf::Predicate {
            name: "repo.has.content".to_string(),
            args: vec![LqPredicateArg::Keyword("corp-a".to_string())],
        }),
        LqExpr::Leaf(LqLeaf::Keyword("shared_oracle_needle".to_string())),
    ]));
    let mut gate_ids: Vec<String> = searcher
        .search(&gate_query, 10)?
        .into_iter()
        .map(|hit| hit.candidate_id)
        .collect();
    gate_ids.sort();
    if gate_ids != vec!["corp-a-doc".to_string(), "corp-a-gate".to_string()] {
        return Err(
            format!("expected corp-a repo.has.content true-gate ids, got {gate_ids:?}").into(),
        );
    }

    let miss_query = make_query(LqExpr::All(vec![
        LqExpr::Leaf(LqLeaf::Predicate {
            name: "repo.has.content".to_string(),
            args: vec![LqPredicateArg::Keyword("missing-corpus-token".to_string())],
        }),
        LqExpr::Leaf(LqLeaf::Keyword("shared_oracle_needle".to_string())),
    ]));
    let miss_hits = searcher.search(&miss_query, 10)?;
    if !miss_hits.is_empty() {
        return Err(format!(
            "expected repo.has.content miss to return zero hits, got {miss_hits:?}"
        )
        .into());
    }

    Ok(())
}

#[test]
fn tantivy_repo_has_content_phrase_and_raw_string_true_gate_narrows_by_indexed_source_repo_id()
-> TestResult {
    let dir = tempfile::tempdir()?;
    let adapter = LexicalAdapter::with_state_root(dir.path().to_path_buf());

    let ops = vec![
        upsert_with_source_repo(
            "corp-a-doc",
            "src/corp-a.rs",
            "rust",
            1,
            2,
            "shared_oracle_needle corp-a branch",
            "corp-a",
        )?,
        upsert_with_source_repo(
            "corp-b-doc",
            "src/corp-b.rs",
            "rust",
            1,
            2,
            "shared_oracle_needle corp-b branch",
            "corp-b",
        )?,
        upsert_with_source_repo(
            "corp-a-gate",
            "src/gate-a.rs",
            "rust",
            1,
            2,
            "shared_oracle_needle gate-a only 123",
            "corp-a",
        )?,
    ];
    adapter.build(&repo(), &revision(), generation(), &ops)?;

    let searcher = adapter.open(&repo(), &revision(), generation())?;

    for args in [
        vec![LqPredicateArg::Phrase("gate-a only".to_string())],
        vec![LqPredicateArg::RawString("gate-a only".to_string())],
    ] {
        let gate_query = make_query(LqExpr::All(vec![
            LqExpr::Leaf(LqLeaf::Predicate {
                name: "repo.has.content".to_string(),
                args,
            }),
            LqExpr::Leaf(LqLeaf::Keyword("shared_oracle_needle".to_string())),
        ]));
        let mut gate_ids: Vec<String> = searcher
            .search(&gate_query, 10)?
            .into_iter()
            .map(|hit| hit.candidate_id)
            .collect();
        gate_ids.sort();
        if gate_ids != vec!["corp-a-doc".to_string(), "corp-a-gate".to_string()] {
            return Err(format!(
                "expected phrase/raw repo.has.content true-gate ids [corp-a-doc, corp-a-gate], got {gate_ids:?}"
            )
            .into());
        }
    }

    Ok(())
}

#[test]
fn tantivy_executes_native_predicate_aliases() -> TestResult {
    let dir = tempfile::tempdir()?;
    let adapter = LexicalAdapter::with_state_root(dir.path().to_path_buf());

    let ops = vec![
        upsert_with_source_repo(
            "corp-a-doc",
            "src/corp-a.rs",
            "rust",
            1,
            2,
            "shared_oracle_needle corp-a branch",
            "corp-a",
        )?,
        upsert_with_source_repo(
            "corp-b-doc",
            "src/corp-b.rs",
            "rust",
            1,
            2,
            "shared_oracle_needle corp-b branch",
            "corp-b",
        )?,
        upsert_with_source_repo(
            "corp-a-gate",
            "src/gate-a.rs",
            "rust",
            1,
            2,
            "shared_oracle_needle gate-a only 123",
            "corp-a",
        )?,
        upsert_with_metadata(
            "phrase_hit",
            "docs/colors.md",
            "markdown",
            3,
            4,
            "the lemon yellow banana ripens",
        )?,
    ];
    adapter.build(&repo(), &revision(), generation(), &ops)?;
    let searcher = adapter.open(&repo(), &revision(), generation())?;

    let repo_path_alias = make_query(LqExpr::All(vec![
        LqExpr::Leaf(LqLeaf::Predicate {
            name: "repo.has.path".to_string(),
            args: vec![LqPredicateArg::Keyword("src/gate-a.rs".to_string())],
        }),
        LqExpr::Leaf(LqLeaf::Keyword("shared_oracle_needle".to_string())),
    ]));
    let mut repo_path_alias_ids: Vec<String> = searcher
        .search(&repo_path_alias, 10)?
        .into_iter()
        .map(|hit| hit.candidate_id)
        .collect();
    repo_path_alias_ids.sort();
    if repo_path_alias_ids != vec!["corp-a-doc".to_string(), "corp-a-gate".to_string()] {
        return Err(format!(
            "expected repo.has.path alias to gate corp-a only, got {repo_path_alias_ids:?}"
        )
        .into());
    }

    let repo_content_alias = make_query(LqExpr::All(vec![
        LqExpr::Leaf(LqLeaf::Predicate {
            name: "repo.contains.content".to_string(),
            args: vec![LqPredicateArg::Phrase("gate-a only".to_string())],
        }),
        LqExpr::Leaf(LqLeaf::Keyword("shared_oracle_needle".to_string())),
    ]));
    let mut repo_content_alias_ids: Vec<String> = searcher
        .search(&repo_content_alias, 10)?
        .into_iter()
        .map(|hit| hit.candidate_id)
        .collect();
    repo_content_alias_ids.sort();
    if repo_content_alias_ids != vec!["corp-a-doc".to_string(), "corp-a-gate".to_string()] {
        return Err(format!(
            "expected repo.contains.content alias to gate corp-a only, got {repo_content_alias_ids:?}"
        )
        .into());
    }

    let file_contains_alias = make_query(LqExpr::Leaf(LqLeaf::Predicate {
        name: "file.contains.content".to_string(),
        args: vec![LqPredicateArg::Phrase("lemon yellow banana".to_string())],
    }));
    let file_contains_alias_ids: Vec<String> = searcher
        .search(&file_contains_alias, 10)?
        .into_iter()
        .map(|hit| hit.candidate_id)
        .collect();
    if file_contains_alias_ids != vec!["phrase_hit".to_string()] {
        return Err(format!(
            "expected file.contains.content alias to hit [phrase_hit], got {file_contains_alias_ids:?}"
        )
        .into());
    }

    Ok(())
}

#[test]
fn tantivy_executes_numeric_content_predicates() -> TestResult {
    let dir = tempfile::tempdir()?;
    let adapter = LexicalAdapter::with_state_root(dir.path().to_path_buf());

    let ops = vec![
        upsert_with_metadata(
            "number_hit",
            "config/path_only_needle.toml",
            "text",
            1,
            2,
            "value = 1",
        )?,
        upsert_with_source_repo(
            "corp-a-doc",
            "src/corp-a.rs",
            "rust",
            3,
            4,
            "shared_oracle_needle corp-a branch",
            "corp-a",
        )?,
        upsert_with_source_repo(
            "corp-b-doc",
            "src/corp-b.rs",
            "rust",
            5,
            6,
            "shared_oracle_needle corp-b branch",
            "corp-b",
        )?,
        upsert_with_source_repo(
            "corp-a-gate",
            "src/gate-a.rs",
            "rust",
            7,
            8,
            "shared_oracle_needle gate-a only 123",
            "corp-a",
        )?,
    ];
    adapter.build(&repo(), &revision(), generation(), &ops)?;
    let searcher = adapter.open(&repo(), &revision(), generation())?;

    for name in ["file.contains", "file.has.content"] {
        let query = make_query(LqExpr::Leaf(LqLeaf::Predicate {
            name: name.to_string(),
            args: vec![LqPredicateArg::Number(1)],
        }));
        let ids: Vec<String> = searcher
            .search(&query, 10)?
            .into_iter()
            .map(|hit| hit.candidate_id)
            .collect();
        if ids != vec!["number_hit".to_string()] {
            return Err(format!("expected {name}(1) to hit [number_hit], got {ids:?}").into());
        }
    }

    let file_has_content_miss = make_query(LqExpr::Leaf(LqLeaf::Predicate {
        name: "file.has.content".to_string(),
        args: vec![LqPredicateArg::Number(404)],
    }));
    let file_has_content_miss_hits = searcher.search(&file_has_content_miss, 10)?;
    if !file_has_content_miss_hits.is_empty() {
        return Err(format!(
            "expected file.has.content(404) to miss, got {file_has_content_miss_hits:?}"
        )
        .into());
    }

    let repo_number_gate = make_query(LqExpr::All(vec![
        LqExpr::Leaf(LqLeaf::Predicate {
            name: "repo.has.content".to_string(),
            args: vec![LqPredicateArg::Number(123)],
        }),
        LqExpr::Leaf(LqLeaf::Keyword("shared_oracle_needle".to_string())),
    ]));
    let mut repo_number_ids: Vec<String> = searcher
        .search(&repo_number_gate, 10)?
        .into_iter()
        .map(|hit| hit.candidate_id)
        .collect();
    repo_number_ids.sort();
    if repo_number_ids != vec!["corp-a-doc".to_string(), "corp-a-gate".to_string()] {
        return Err(format!(
            "expected repo.has.content(123) to gate corp-a only, got {repo_number_ids:?}"
        )
        .into());
    }

    Ok(())
}

#[test]
fn tantivy_executes_repo_has_file_predicate_as_repo_gate() -> TestResult {
    let dir = tempfile::tempdir()?;
    let adapter = LexicalAdapter::with_state_root(dir.path().to_path_buf());

    let ops = vec![
        upsert_with_metadata("alpha", "src/lib.rs", "rust", 4, 8, "needle alpha")?,
        upsert_with_metadata("beta", "src/main.rs", "rust", 10, 12, "needle beta")?,
        upsert_with_metadata("gamma", "docs/readme.md", "markdown", 1, 2, "no match")?,
    ];
    adapter.build(&repo(), &revision(), generation(), &ops)?;

    let searcher = adapter.open(&repo(), &revision(), generation())?;
    let gate_query = make_query(LqExpr::All(vec![
        LqExpr::Leaf(LqLeaf::Predicate {
            name: "repo.has.file".to_string(),
            args: vec![LqPredicateArg::Filter {
                name: "path".to_string(),
                value: "src/lib.rs".to_string(),
            }],
        }),
        LqExpr::Leaf(LqLeaf::Keyword("needle".to_string())),
    ]));
    let gate_hits = searcher.search(&gate_query, 10)?;
    if gate_hits.len() != 2 {
        return Err(format!("expected 2 repo-gated hits, got {}", gate_hits.len()).into());
    }

    let miss_query = make_query(LqExpr::All(vec![
        LqExpr::Leaf(LqLeaf::Predicate {
            name: "repo.has.file".to_string(),
            args: vec![LqPredicateArg::Filter {
                name: "path".to_string(),
                value: "missing.rs".to_string(),
            }],
        }),
        LqExpr::Leaf(LqLeaf::Keyword("needle".to_string())),
    ]));
    let miss_hits = searcher.search(&miss_query, 10)?;
    if !miss_hits.is_empty() {
        return Err(format!("expected 0 repo-gated hits, got {miss_hits:?}").into());
    }

    Ok(())
}

#[test]
fn tantivy_executes_repo_has_content_predicate_under_or_and_not() -> TestResult {
    let dir = tempfile::tempdir()?;
    let adapter = LexicalAdapter::with_state_root(dir.path().to_path_buf());

    let ops = vec![
        upsert_with_source_repo(
            "corp-a-doc",
            "src/corp-a.rs",
            "rust",
            1,
            2,
            "shared_oracle_needle corp-a branch",
            "corp-a",
        )?,
        upsert_with_source_repo(
            "corp-b-doc",
            "src/corp-b.rs",
            "rust",
            1,
            2,
            "shared_oracle_needle corp-b branch",
            "corp-b",
        )?,
        upsert_with_source_repo(
            "corp-a-gate",
            "src/gate-a.rs",
            "rust",
            1,
            2,
            "shared_oracle_needle gate-a only 123",
            "corp-a",
        )?,
    ];
    adapter.build(&repo(), &revision(), generation(), &ops)?;

    let searcher = adapter.open(&repo(), &revision(), generation())?;

    let or_query = make_query(LqExpr::Any(vec![
        LqExpr::Leaf(LqLeaf::Predicate {
            name: "repo.has.content".to_string(),
            args: vec![LqPredicateArg::Phrase("gate-a only".to_string())],
        }),
        LqExpr::Leaf(LqLeaf::Keyword("missing-corpus-token".to_string())),
    ]));
    let mut or_ids: Vec<String> = searcher
        .search(&or_query, 10)?
        .into_iter()
        .map(|candidate| candidate.candidate_id)
        .collect();
    or_ids.sort();
    if or_ids != vec!["corp-a-doc".to_string(), "corp-a-gate".to_string()] {
        return Err(format!(
            "expected OR repo.has.content ids [corp-a-doc, corp-a-gate], got {or_ids:?}"
        )
        .into());
    }

    let not_true_query = make_query(LqExpr::All(vec![
        LqExpr::Not(Box::new(LqExpr::Leaf(LqLeaf::Predicate {
            name: "repo.has.content".to_string(),
            args: vec![LqPredicateArg::Phrase("gate-a only".to_string())],
        }))),
        LqExpr::Leaf(LqLeaf::Keyword("shared_oracle_needle".to_string())),
    ]));
    let mut not_true_ids: Vec<String> = searcher
        .search(&not_true_query, 10)?
        .into_iter()
        .map(|candidate| candidate.candidate_id)
        .collect();
    not_true_ids.sort();
    if not_true_ids != vec!["corp-b-doc".to_string()] {
        return Err(format!(
            "expected NOT(true repo.has.content) ids [corp-b-doc], got {not_true_ids:?}"
        )
        .into());
    }

    let not_false_query = make_query(LqExpr::All(vec![
        LqExpr::Not(Box::new(LqExpr::Leaf(LqLeaf::Predicate {
            name: "repo.has.content".to_string(),
            args: vec![LqPredicateArg::Keyword("missing-corpus-token".to_string())],
        }))),
        LqExpr::Leaf(LqLeaf::Keyword("needle".to_string())),
    ]));
    let mut not_false_ids: Vec<String> = searcher
        .search(&not_false_query, 10)?
        .into_iter()
        .map(|candidate| candidate.candidate_id)
        .collect();
    not_false_ids.sort();
    if not_false_ids
        != vec![
            "corp-a-doc".to_string(),
            "corp-a-gate".to_string(),
            "corp-b-doc".to_string(),
        ]
    {
        return Err(format!(
            "expected NOT(false repo.has.content) ids [corp-a-doc, corp-a-gate, corp-b-doc], got {not_false_ids:?}"
        )
        .into());
    }

    Ok(())
}

#[test]
fn tantivy_executes_repo_has_file_predicate_with_lang_matcher() -> TestResult {
    // ADV-01 widened arg-shape family: `repo.has.file(lang:<x>)` gates by
    // whether the repo contains a file in language <x>, lowered to one
    // canonical language-field matcher (no ambiguity).
    let dir = tempfile::tempdir()?;
    let adapter = LexicalAdapter::with_state_root(dir.path().to_path_buf());

    let ops = vec![
        upsert_with_metadata("alpha", "src/lib.rs", "rust", 4, 8, "needle alpha")?,
        upsert_with_metadata("beta", "src/app.py", "python", 10, 12, "needle beta")?,
        upsert_with_metadata("gamma", "docs/readme.md", "markdown", 1, 2, "no match")?,
    ];
    adapter.build(&repo(), &revision(), generation(), &ops)?;
    let searcher = adapter.open(&repo(), &revision(), generation())?;

    // The repo contains a python file -> gate opens, both `needle` docs return.
    let hit_query = make_query(LqExpr::All(vec![
        LqExpr::Leaf(LqLeaf::Predicate {
            name: "repo.has.file".to_string(),
            args: vec![LqPredicateArg::Filter {
                name: "lang".to_string(),
                value: "python".to_string(),
            }],
        }),
        LqExpr::Leaf(LqLeaf::Keyword("needle".to_string())),
    ]));
    let hits = searcher.search(&hit_query, 10)?;
    if hits.len() != 2 {
        return Err(format!(
            "expected 2 lang-gated hits (repo has a python file), got {}",
            hits.len()
        )
        .into());
    }

    // No Go file exists -> gate closes -> zero hits (fail-closed, not a fallback).
    let miss_query = make_query(LqExpr::All(vec![
        LqExpr::Leaf(LqLeaf::Predicate {
            name: "repo.has.file".to_string(),
            args: vec![LqPredicateArg::Filter {
                name: "lang".to_string(),
                value: "go".to_string(),
            }],
        }),
        LqExpr::Leaf(LqLeaf::Keyword("needle".to_string())),
    ]));
    let miss = searcher.search(&miss_query, 10)?;
    if !miss.is_empty() {
        return Err(format!("expected 0 hits (no go file in repo), got {miss:?}").into());
    }
    Ok(())
}

#[test]
fn tantivy_executes_repo_has_file_predicate_under_or_and_not() -> TestResult {
    let dir = tempfile::tempdir()?;
    let adapter = LexicalAdapter::with_state_root(dir.path().to_path_buf());

    let ops = vec![
        upsert_with_source_repo(
            "corp-a-doc",
            "src/corp-a.rs",
            "rust",
            1,
            2,
            "shared_oracle_needle corp-a branch",
            "corp-a",
        )?,
        upsert_with_source_repo(
            "corp-b-doc",
            "src/corp-b.rs",
            "rust",
            1,
            2,
            "shared_oracle_needle corp-b branch",
            "corp-b",
        )?,
        upsert_with_source_repo(
            "corp-a-gate",
            "src/gate-a.rs",
            "rust",
            1,
            2,
            "shared_oracle_needle gate-a only 123",
            "corp-a",
        )?,
    ];
    adapter.build(&repo(), &revision(), generation(), &ops)?;

    let searcher = adapter.open(&repo(), &revision(), generation())?;

    let or_query = make_query(LqExpr::Any(vec![
        LqExpr::Leaf(LqLeaf::Predicate {
            name: "repo.has.file".to_string(),
            args: vec![LqPredicateArg::Filter {
                name: "path".to_string(),
                value: "src/gate-a.rs".to_string(),
            }],
        }),
        LqExpr::Leaf(LqLeaf::Keyword("missing-corpus-token".to_string())),
    ]));
    let mut or_ids: Vec<String> = searcher
        .search(&or_query, 10)?
        .into_iter()
        .map(|candidate| candidate.candidate_id)
        .collect();
    or_ids.sort();
    if or_ids != vec!["corp-a-doc".to_string(), "corp-a-gate".to_string()] {
        return Err(format!(
            "expected OR repo.has.file ids [corp-a-doc, corp-a-gate], got {or_ids:?}"
        )
        .into());
    }

    let not_true_query = make_query(LqExpr::All(vec![
        LqExpr::Not(Box::new(LqExpr::Leaf(LqLeaf::Predicate {
            name: "repo.has.file".to_string(),
            args: vec![LqPredicateArg::Filter {
                name: "path".to_string(),
                value: "src/gate-a.rs".to_string(),
            }],
        }))),
        LqExpr::Leaf(LqLeaf::Keyword("shared_oracle_needle".to_string())),
    ]));
    let mut not_true_ids: Vec<String> = searcher
        .search(&not_true_query, 10)?
        .into_iter()
        .map(|candidate| candidate.candidate_id)
        .collect();
    not_true_ids.sort();
    if not_true_ids != vec!["corp-b-doc".to_string()] {
        return Err(format!(
            "expected NOT(true repo.has.file) ids [corp-b-doc], got {not_true_ids:?}"
        )
        .into());
    }

    let not_false_query = make_query(LqExpr::All(vec![
        LqExpr::Not(Box::new(LqExpr::Leaf(LqLeaf::Predicate {
            name: "repo.has.file".to_string(),
            args: vec![LqPredicateArg::Filter {
                name: "path".to_string(),
                value: "missing.rs".to_string(),
            }],
        }))),
        LqExpr::Leaf(LqLeaf::Keyword("needle".to_string())),
    ]));
    let mut not_false_ids: Vec<String> = searcher
        .search(&not_false_query, 10)?
        .into_iter()
        .map(|candidate| candidate.candidate_id)
        .collect();
    not_false_ids.sort();
    if not_false_ids
        != vec![
            "corp-a-doc".to_string(),
            "corp-a-gate".to_string(),
            "corp-b-doc".to_string(),
        ]
    {
        return Err(format!(
            "expected NOT(false repo.has.file) ids [corp-a-doc, corp-a-gate, corp-b-doc], got {not_false_ids:?}"
        )
        .into());
    }

    Ok(())
}

#[test]
fn tantivy_executes_file_has_content_predicate_phrase_and_regex() -> TestResult {
    let dir = tempfile::tempdir()?;
    let adapter = LexicalAdapter::with_state_root(dir.path().to_path_buf());

    let ops = vec![
        upsert_with_metadata(
            "phrase_hit",
            "docs/colors.md",
            "markdown",
            1,
            2,
            "the lemon yellow banana ripens",
        )?,
        upsert_with_metadata(
            "phrase_miss",
            "docs/other.md",
            "markdown",
            3,
            4,
            "banana near lemon but not adjacent",
        )?,
        upsert_with_metadata(
            "regex_hit",
            "src/version.rs",
            "rust",
            10,
            11,
            "const VERSION: &str = \"v1.2.3-rc.4\";",
        )?,
        upsert_with_metadata(
            "regex_miss",
            "src/plain.rs",
            "rust",
            12,
            13,
            "no version token here",
        )?,
        upsert_with_metadata(
            "raw_hit",
            "src/raw.rs",
            "rust",
            14,
            15,
            "let foo_bar_baz = 0;",
        )?,
    ];
    adapter.build(&repo(), &revision(), generation(), &ops)?;

    let searcher = adapter.open(&repo(), &revision(), generation())?;

    let phrase_hit_query = make_query(LqExpr::Leaf(LqLeaf::Predicate {
        name: "file.has.content".to_string(),
        args: vec![LqPredicateArg::Phrase("lemon yellow banana".to_string())],
    }));
    let phrase_hit_ids: Vec<String> = searcher
        .search(&phrase_hit_query, 10)?
        .into_iter()
        .map(|candidate| candidate.candidate_id)
        .collect();
    if phrase_hit_ids != vec!["phrase_hit".to_string()] {
        return Err(format!(
            "expected file.has.content phrase predicate to hit [phrase_hit], got {phrase_hit_ids:?}"
        )
        .into());
    }

    let phrase_miss_query = make_query(LqExpr::Leaf(LqLeaf::Predicate {
        name: "file.has.content".to_string(),
        args: vec![LqPredicateArg::Phrase("banana lemon".to_string())],
    }));
    let phrase_miss_hits = searcher.search(&phrase_miss_query, 10)?;
    if !phrase_miss_hits.is_empty() {
        return Err(format!(
            "expected file.has.content reversed phrase predicate to miss, got {phrase_miss_hits:?}"
        )
        .into());
    }

    let regex_hit_query = make_query(LqExpr::Leaf(LqLeaf::Predicate {
        name: "file.has.content".to_string(),
        args: vec![LqPredicateArg::Keyword("/v\\d+\\.\\d+\\.\\d+/".to_string())],
    }));
    let regex_hit_ids: Vec<String> = searcher
        .search(&regex_hit_query, 10)?
        .into_iter()
        .map(|candidate| candidate.candidate_id)
        .collect();
    if regex_hit_ids != vec!["regex_hit".to_string()] {
        return Err(format!(
            "expected file.has.content regex predicate to hit [regex_hit], got {regex_hit_ids:?}"
        )
        .into());
    }

    let contains_hit_query = make_query(LqExpr::Leaf(LqLeaf::Predicate {
        name: "file.contains".to_string(),
        args: vec![LqPredicateArg::RawString("oo_ba".to_string())],
    }));
    let contains_hit_ids: Vec<String> = searcher
        .search(&contains_hit_query, 10)?
        .into_iter()
        .map(|candidate| candidate.candidate_id)
        .collect();
    if contains_hit_ids != vec!["raw_hit".to_string()] {
        return Err(format!(
            "expected file.contains raw predicate to hit [raw_hit], got {contains_hit_ids:?}"
        )
        .into());
    }

    let contains_phrase_hit_query = make_query(LqExpr::Leaf(LqLeaf::Predicate {
        name: "file.contains".to_string(),
        args: vec![LqPredicateArg::Phrase("lemon yellow banana".to_string())],
    }));
    let contains_phrase_hit_ids: Vec<String> = searcher
        .search(&contains_phrase_hit_query, 10)?
        .into_iter()
        .map(|candidate| candidate.candidate_id)
        .collect();
    if contains_phrase_hit_ids != vec!["phrase_hit".to_string()] {
        return Err(format!(
            "expected file.contains phrase predicate to hit [phrase_hit], got {contains_phrase_hit_ids:?}"
        )
        .into());
    }

    // Distinct predicate name from `file.contains`; same substrate must not
    // widen to unrelated docs when the pattern misses.
    let contains_miss_query = make_query(LqExpr::Leaf(LqLeaf::Predicate {
        name: "file.contains".to_string(),
        args: vec![LqPredicateArg::Phrase("banana lemon".to_string())],
    }));
    let contains_miss_hits = searcher.search(&contains_miss_query, 10)?;
    if !contains_miss_hits.is_empty() {
        return Err(format!(
            "expected file.contains reversed phrase predicate to miss, got {contains_miss_hits:?}"
        )
        .into());
    }

    Ok(())
}

#[test]
fn tantivy_executes_scoped_file_content_predicates_and_fails_closed_in_or_not() -> TestResult {
    let dir = tempfile::tempdir()?;
    let adapter = LexicalAdapter::with_state_root(dir.path().to_path_buf());

    let ops = vec![
        upsert_with_metadata(
            "phrase_hit",
            "docs/colors.md",
            "markdown",
            1,
            2,
            "the lemon yellow banana ripens",
        )?,
        upsert_with_metadata(
            "phrase_miss",
            "docs/other.md",
            "markdown",
            3,
            4,
            "banana near lemon but not adjacent",
        )?,
        upsert_with_metadata(
            "regex_hit",
            "src/version.rs",
            "rust",
            10,
            11,
            "const VERSION: &str = \"v1.2.3-rc.4\";",
        )?,
        upsert_with_metadata(
            "alpha",
            "src/lib.rs",
            "rust",
            12,
            13,
            "fn alpha_content_needle() {}",
        )?,
    ];
    adapter.build(&repo(), &revision(), generation(), &ops)?;
    let searcher = adapter.open(&repo(), &revision(), generation())?;

    for (args, expected) in [
        (
            vec![
                LqPredicateArg::Filter {
                    name: "path".to_string(),
                    value: "docs/colors.md".to_string(),
                },
                LqPredicateArg::Phrase("lemon yellow banana".to_string()),
            ],
            vec!["phrase_hit".to_string()],
        ),
        (
            vec![
                LqPredicateArg::Filter {
                    name: "file".to_string(),
                    value: "colors.md".to_string(),
                },
                LqPredicateArg::Phrase("lemon yellow banana".to_string()),
            ],
            vec!["phrase_hit".to_string()],
        ),
        (
            vec![
                LqPredicateArg::Filter {
                    name: "lang".to_string(),
                    value: "rust".to_string(),
                },
                LqPredicateArg::Keyword("/v\\d+\\.\\d+\\.\\d+/".to_string()),
            ],
            vec!["regex_hit".to_string()],
        ),
    ] {
        let query = make_query(LqExpr::Leaf(LqLeaf::Predicate {
            name: "file.has.content".to_string(),
            args: args.clone(),
        }));
        let ids: Vec<String> = searcher
            .search(&query, 10)?
            .into_iter()
            .map(|hit| hit.candidate_id)
            .collect();
        if ids != expected {
            return Err(
                format!("expected scoped file.has.content hits {expected:?}, got {ids:?}").into(),
            );
        }
    }

    let file_miss_query = make_query(LqExpr::Leaf(LqLeaf::Predicate {
        name: "file.contains".to_string(),
        args: vec![
            LqPredicateArg::Filter {
                name: "file".to_string(),
                value: "missing.md".to_string(),
            },
            LqPredicateArg::Phrase("lemon yellow banana".to_string()),
        ],
    }));
    let file_miss_hits = searcher.search(&file_miss_query, 10)?;
    if !file_miss_hits.is_empty() {
        return Err(format!(
            "expected scoped file.contains(file:missing.md, ...) to miss, got {file_miss_hits:?}"
        )
        .into());
    }

    let lang_miss_query = make_query(LqExpr::Leaf(LqLeaf::Predicate {
        name: "file.has.content".to_string(),
        args: vec![
            LqPredicateArg::Filter {
                name: "lang".to_string(),
                value: "markdown".to_string(),
            },
            LqPredicateArg::Keyword("/v\\d+\\.\\d+\\.\\d+/".to_string()),
        ],
    }));
    let lang_miss_hits = searcher.search(&lang_miss_query, 10)?;
    if !lang_miss_hits.is_empty() {
        return Err(format!(
            "expected scoped file.has.content(lang:markdown, /v.../) to miss, got {lang_miss_hits:?}"
        )
        .into());
    }

    let and_query = make_query(LqExpr::All(vec![
        LqExpr::Leaf(LqLeaf::Predicate {
            name: "file.contains".to_string(),
            args: vec![
                LqPredicateArg::Filter {
                    name: "path".to_string(),
                    value: "docs/colors.md".to_string(),
                },
                LqPredicateArg::Phrase("lemon yellow banana".to_string()),
            ],
        }),
        LqExpr::Leaf(LqLeaf::Keyword("ripens".to_string())),
    ]));
    let and_ids: Vec<String> = searcher
        .search(&and_query, 10)?
        .into_iter()
        .map(|hit| hit.candidate_id)
        .collect();
    if and_ids != vec!["phrase_hit".to_string()] {
        return Err(format!(
            "expected scoped file.contains AND to hit [phrase_hit], got {and_ids:?}"
        )
        .into());
    }

    let and_miss_query = make_query(LqExpr::All(vec![
        LqExpr::Leaf(LqLeaf::Predicate {
            name: "file.contains".to_string(),
            args: vec![
                LqPredicateArg::Filter {
                    name: "path".to_string(),
                    value: "src/lib.rs".to_string(),
                },
                LqPredicateArg::Phrase("lemon yellow banana".to_string()),
            ],
        }),
        LqExpr::Leaf(LqLeaf::Keyword("ripens".to_string())),
    ]));
    let and_miss_ids: Vec<String> = searcher
        .search(&and_miss_query, 10)?
        .into_iter()
        .map(|hit| hit.candidate_id)
        .collect();
    if !and_miss_ids.is_empty() {
        return Err(format!(
            "expected scoped file.contains(path:src/lib.rs, ...) AND ripens to miss, got {and_miss_ids:?}"
        )
        .into());
    }

    let or_query = make_query(LqExpr::Any(vec![
        LqExpr::Leaf(LqLeaf::Predicate {
            name: "file.contains".to_string(),
            args: vec![
                LqPredicateArg::Filter {
                    name: "path".to_string(),
                    value: "docs/colors.md".to_string(),
                },
                LqPredicateArg::Phrase("lemon yellow banana".to_string()),
            ],
        }),
        LqExpr::Leaf(LqLeaf::Keyword("alpha_content_needle".to_string())),
    ]));
    match searcher.search(&or_query, 10) {
        Err(CoreError::Typed { code, .. })
            if code == "LEX_PREDICATE_SCOPED_BOOLEAN_UNSUPPORTED" => {}
        other => {
            return Err(format!(
                "expected scoped file.contains OR to fail closed with LEX_PREDICATE_SCOPED_BOOLEAN_UNSUPPORTED, got {other:?}"
            )
            .into());
        }
    }

    let not_query = make_query(LqExpr::Not(Box::new(LqExpr::Leaf(LqLeaf::Predicate {
        name: "file.contains".to_string(),
        args: vec![
            LqPredicateArg::Filter {
                name: "path".to_string(),
                value: "docs/colors.md".to_string(),
            },
            LqPredicateArg::Phrase("lemon yellow banana".to_string()),
        ],
    }))));
    match searcher.search(&not_query, 10) {
        Err(CoreError::Typed { code, .. })
            if code == "LEX_PREDICATE_SCOPED_BOOLEAN_UNSUPPORTED" => {}
        other => {
            return Err(format!(
                "expected scoped file.contains NOT to fail closed with LEX_PREDICATE_SCOPED_BOOLEAN_UNSUPPORTED, got {other:?}"
            )
            .into());
        }
    }

    let bad_matcher_query = make_query(LqExpr::Leaf(LqLeaf::Predicate {
        name: "file.contains".to_string(),
        args: vec![
            LqPredicateArg::Filter {
                name: "name".to_string(),
                value: "colors.md".to_string(),
            },
            LqPredicateArg::Phrase("lemon yellow banana".to_string()),
        ],
    }));
    match searcher.search(&bad_matcher_query, 10) {
        Err(CoreError::Typed { code, .. }) if code == "LEX_PREDICATE_UNIMPLEMENTED" => {}
        other => {
            return Err(format!(
                "expected file.contains(name:...) to fail closed with LEX_PREDICATE_UNIMPLEMENTED, got {other:?}"
            )
            .into());
        }
    }

    let multiple_scalars_query = make_query(LqExpr::Leaf(LqLeaf::Predicate {
        name: "file.contains".to_string(),
        args: vec![
            LqPredicateArg::Phrase("lemon".to_string()),
            LqPredicateArg::Phrase("banana".to_string()),
        ],
    }));
    match searcher.search(&multiple_scalars_query, 10) {
        Err(CoreError::Typed { code, .. }) if code == "LEX_PREDICATE_UNIMPLEMENTED" => {}
        other => {
            return Err(format!(
                "expected file.contains(two scalars) to fail closed with LEX_PREDICATE_UNIMPLEMENTED, got {other:?}"
            )
            .into());
        }
    }

    let repo_scoped_query = make_query(LqExpr::Leaf(LqLeaf::Predicate {
        name: "repo.has.content".to_string(),
        args: vec![
            LqPredicateArg::Filter {
                name: "path".to_string(),
                value: "src".to_string(),
            },
            LqPredicateArg::Number(7),
        ],
    }));
    match searcher.search(&repo_scoped_query, 10) {
        Err(CoreError::Typed { code, .. }) if code == "LEX_PREDICATE_UNIMPLEMENTED" => {}
        other => {
            return Err(format!(
                "expected repo.has.content(path:src, 7) to fail closed with LEX_PREDICATE_UNIMPLEMENTED, got {other:?}"
            )
            .into());
        }
    }

    Ok(())
}

#[test]
fn tantivy_search_all_materializes_full_scope() -> TestResult {
    let dir = tempfile::tempdir()?;
    let adapter = LexicalAdapter::with_state_root(dir.path().to_path_buf());

    let ops = vec![
        upsert("c1", "scope needle one")?,
        upsert("c2", "scope needle two")?,
        upsert("c3", "scope needle three")?,
    ];
    adapter.build(&repo(), &revision(), generation(), &ops)?;

    let searcher = adapter.open(&repo(), &revision(), generation())?;
    let hits = searcher.search_all(&make_query(LqExpr::Leaf(LqLeaf::Keyword(
        "scope".to_string(),
    ))))?;
    let mut ids: Vec<String> = hits.into_iter().map(|c| c.candidate_id).collect();
    ids.sort();
    if ids != vec!["c1".to_string(), "c2".to_string(), "c3".to_string()] {
        return Err(format!("expected full scope ids [c1, c2, c3], got {ids:?}").into());
    }
    Ok(())
}

/// LRU eviction hellgate.
///
/// Drive `LEXICAL_WRITER_CACHE_MAX + 1` distinct generations through the
/// adapter, then assert
///   (a) the writer cache holds at most `LEXICAL_WRITER_CACHE_MAX` entries, and
///   (b) the LRU victim (generation 0, the first inserted) was committed before
///       eviction — verified by opening a searcher on it and finding the chunk.
///
/// (a) defends the memory bound described in the `WriterCache` doc-comment.
/// (b) defends the commit-on-eviction guarantee.
#[test]
fn writer_cache_evicts_lru_after_threshold() -> TestResult {
    let dir = tempfile::tempdir()?;
    let adapter = LexicalAdapter::with_state_root(dir.path().to_path_buf());

    let total: u64 = u64::try_from(LEXICAL_WRITER_CACHE_MAX)? + 1;

    for gen_index in 0..total {
        let generation = ManifestGeneration::new(gen_index);
        let chunk_id = format!("chunk-g{gen_index}");
        let text = format!("eviction-marker generation-{gen_index}");
        let op = LexicalChannelOp::UpsertChunk(UpsertChunk {
            repo_id: repo(),
            revision_id: revision(),
            generation,
            chunk_id: ChunkId::new(&chunk_id),
            payload: encode_chunk_payload(&chunk_id, &text)?,
        });
        adapter.build(&repo(), &revision(), generation, &[op])?;
    }

    let cached = adapter.writer_cache_len()?;
    if cached > LEXICAL_WRITER_CACHE_MAX {
        return Err(format!(
            "writer cache holds {cached} entries, expected <= {LEXICAL_WRITER_CACHE_MAX}"
        )
        .into());
    }

    // Generation 0 is the oldest insert and must have been the LRU victim once
    // we crossed the cap on the 17th insert. Opening a fresh searcher on it
    // exercises the on-disk index (the writer was committed on eviction, then
    // dropped, so this read goes through `open_or_create_index`, not the cache).
    let gen0 = ManifestGeneration::new(0);
    let searcher = adapter.open(&repo(), &revision(), gen0)?;
    let hits = searcher.search(
        &make_query(LqExpr::Leaf(LqLeaf::Keyword("eviction-marker".to_string()))),
        10,
    )?;
    if hits.len() != 1 {
        return Err(format!(
            "expected 1 hit on evicted generation 0 after commit-on-evict, got {}",
            hits.len()
        )
        .into());
    }
    let first = hits.first().ok_or("hits empty after length check")?;
    if first.candidate_id != "chunk-g0" {
        return Err(format!("expected candidate_id chunk-g0, got {}", first.candidate_id).into());
    }

    Ok(())
}
