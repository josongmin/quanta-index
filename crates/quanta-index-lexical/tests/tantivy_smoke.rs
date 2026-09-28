//! Smoke test for the Tantivy-backed lexical adapter.
//!
//! Builds an index from a sealed batch of `UpsertChunk` ops, opens a searcher,
//! and verifies BM25 scoring + boolean composition both return the expected
//! candidate sets. Follows the `wal_roundtrip.rs` test idiom: returns
//! `Result<(), Box<dyn Error>>` and propagates errors via `?` (no `.unwrap()`
//! or `.expect()` per the workspace lint policy).

#![forbid(unsafe_code)]
#![expect(
    clippy::panic_in_result_fn,
    reason = "Result-returning smoke tests assert with `assert!` on fixture invariants; a violated fixture invariant is not a propagatable error"
)]

#[path = "support/op_fixture.rs"]
mod op_fixture;
#[path = "support/source_fixture.rs"]
mod source_fixture;

use std::error::Error;

use quanta_index_contract::channel::LexicalChannelOp;
use quanta_index_contract::lex::{
    LanguageCode, SymbolKindCode, SymbolKindFamily, SymbolRecord, SymbolRelationship, SymbolSpan,
};
use quanta_index_contract::{
    BatchIngestMode, ChunkId, ChunkRecord, ExactRepoRelativePathV1, LQ_VERSION_TAG,
    LexicalFullBundle, LqCase, LqCountBound, LqExpr, LqFileScope, LqFilter, LqLeaf, LqOptions,
    LqPatternType, LqPredicateArg, LqQuery, LqSelect, LqSpan, LqType, LqVisibility, LqYesNoOnly,
    ManifestGeneration, QueryConstraintSetV1, RepoId, RepoRelativePath, RevisionId,
    SearchScopeSurface, SymbolId, UpsertChunk, UpsertSymbol,
};
use quanta_index_core::{
    CoreError, LexicalIndexOpenPort, LexicalPageSpec, RequestBudgetV1, SearchCorpusBatchBuildPort,
};
use quanta_index_lexical::LexicalAdapter;

type TestResult = Result<(), Box<dyn Error>>;

#[derive(Clone, Debug)]
struct RepoMetadataPayload {
    fork: bool,
    archived: bool,
    visibility: LqVisibility,
    contexts: Vec<String>,
}

#[expect(
    clippy::expect_used,
    reason = "static fixture IDs provably satisfy the canonical ID policy"
)]
fn repo() -> RepoId {
    RepoId::new("smoke-repo").expect("static fixture ID satisfies canonical policy")
}

#[expect(
    clippy::expect_used,
    reason = "static fixture IDs provably satisfy the canonical ID policy"
)]
fn revision() -> RevisionId {
    RevisionId::new("smoke-rev").expect("static fixture ID satisfies canonical policy")
}

fn generation() -> ManifestGeneration {
    ManifestGeneration::new(1)
}

/// Author canonical source files from the compact test input before ingestion.
/// The helper binds raw bytes, unit sets, and source publication in one batch.
trait SealedFixtureBuildPort {
    fn build(
        &self,
        repo: &RepoId,
        revision: &RevisionId,
        generation: ManifestGeneration,
        ops: &[LexicalChannelOp],
    ) -> Result<(), CoreError>;
}

impl SealedFixtureBuildPort for LexicalAdapter {
    fn build(
        &self,
        repo: &RepoId,
        revision: &RevisionId,
        generation: ManifestGeneration,
        ops: &[LexicalChannelOp],
    ) -> Result<(), CoreError> {
        SearchCorpusBatchBuildPort::build_batch(
            self,
            &op_fixture::batch(repo, revision, generation, ops)?,
        )
    }
}

fn language_code(code: &str) -> Result<LanguageCode, Box<dyn Error>> {
    LanguageCode::new(code).map_err(|err| -> Box<dyn Error> {
        format!("invalid language code `{code}`: {err}").into()
    })
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

#[expect(
    clippy::expect_used,
    reason = "static fixture IDs provably satisfy the canonical ID policy"
)]
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
        source_repo_id: Some(
            RepoId::new(source_repo_id).expect("test fixture ID satisfies canonical policy"),
        ),
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
        &RequestBudgetV1::unbounded(),
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
        &RequestBudgetV1::unbounded(),
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
    let all_hits = searcher.search(&make_query(all_expr), 10, &RequestBudgetV1::unbounded())?;
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
        &RequestBudgetV1::unbounded(),
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
    let regexp_option_hits =
        searcher.search(&regexp_option_query, 10, &RequestBudgetV1::unbounded())?;
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
        &RequestBudgetV1::unbounded(),
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
fn language_constraint_is_pushed_into_one_pre_limit_candidate_query_v1() -> TestResult {
    let dir = tempfile::tempdir()?;
    let adapter = LexicalAdapter::with_state_root(dir.path().to_path_buf());
    let ops = vec![
        upsert_with_metadata(
            "python-1",
            "src/a.py",
            "python",
            1,
            1,
            "needle needle needle",
        )?,
        upsert_with_metadata(
            "python-2",
            "src/b.py",
            "python",
            1,
            1,
            "needle needle needle",
        )?,
        upsert_with_metadata("rust-target", "src/lib.rs", "rust", 1, 1, "needle")?,
    ];
    adapter.build(&repo(), &revision(), generation(), &ops)?;
    let searcher = adapter.open(&repo(), &revision(), generation())?;

    let query = make_query(LqExpr::Leaf(LqLeaf::Keyword("needle".to_string())));

    let rust_only = QueryConstraintSetV1::from_languages([language_code("rust")?]);
    let hits = searcher
        .search_constrained(
            &query,
            &rust_only,
            &LexicalPageSpec::first(1),
            &RequestBudgetV1::unbounded(),
        )?
        .candidates;
    assert_eq!(
        hits.iter()
            .map(|candidate| candidate.candidate_id.as_str())
            .collect::<Vec<_>>(),
        vec!["rust-target"],
        "language filtering after top_k would incorrectly lose the lower-scoring rust candidate"
    );

    let either = QueryConstraintSetV1::from_languages([
        language_code("rust")?,
        language_code("python")?,
        language_code("rust")?,
    ]);
    let hits = searcher
        .search_constrained(
            &query,
            &either,
            &LexicalPageSpec::first(10),
            &RequestBudgetV1::unbounded(),
        )?
        .candidates;
    assert_eq!(
        hits.len(),
        3,
        "OR-set constraints must not execute as an intersection"
    );
    Ok(())
}

#[test]
fn exact_path_constraint_is_applied_before_limit_and_on_index_no_scan_v1() -> TestResult {
    let dir = tempfile::tempdir()?;
    let adapter = LexicalAdapter::with_state_root(dir.path().to_path_buf());
    let ops = vec![
        upsert_with_metadata(
            "wrong-best",
            "other/lib.rs",
            "rust",
            1,
            1,
            "needle needle needle",
        )?,
        upsert_with_metadata("requested", "src/lib.rs", "rust", 1, 1, "needle")?,
        upsert_with_metadata("unrelated", "src/main.rs", "rust", 1, 1, "needle")?,
        upsert_symbol("wrong-symbol", "other/lib.rs", "rust", "needle", 1, 1)?,
        upsert_symbol("requested-symbol", "src/lib.rs", "rust", "needle", 1, 1)?,
    ];
    adapter.build(&repo(), &revision(), generation(), &ops)?;
    let searcher = adapter.open(&repo(), &revision(), generation())?;
    let constraints = QueryConstraintSetV1::from_exact_repo_relative_path(
        ExactRepoRelativePathV1::new("src/lib.rs").map_err(str::to_string)?,
    );
    let query = make_query(LqExpr::Leaf(LqLeaf::Keyword("needle".to_string())));

    let indexed = searcher
        .search_constrained(
            &query,
            &constraints,
            &LexicalPageSpec::first(1),
            &RequestBudgetV1::unbounded(),
        )?
        .candidates;
    assert_eq!(
        indexed
            .iter()
            .map(|candidate| candidate.candidate_id.as_str())
            .collect::<Vec<_>>(),
        vec!["requested"],
        "post-limit filtering would lose the lower-scoring exact-path candidate"
    );

    let symbols = searcher.search_symbols_constrained(
        &query,
        &constraints,
        &LexicalPageSpec::first(1),
        &RequestBudgetV1::unbounded(),
    )?;
    assert_eq!(
        symbols
            .candidates
            .iter()
            .map(|candidate| candidate.candidate_id.as_str())
            .collect::<Vec<_>>(),
        vec!["requested-symbol"],
        "symbol ranking must receive the same exact-path predicate before top_k"
    );

    let capped = searcher
        .search_constrained(
            &query,
            &constraints,
            &LexicalPageSpec::first(16),
            &RequestBudgetV1::unbounded(),
        )?
        .candidates;
    assert_eq!(
        capped
            .iter()
            .map(|candidate| candidate.candidate_id.as_str())
            .collect::<Vec<_>>(),
        vec!["requested"],
        "a capped scope materialization must not admit another same-basename path"
    );

    let mut unindexed = query;
    unindexed.options.index_mode = Some(LqYesNoOnly::No);
    let scanned = searcher
        .search_constrained(
            &unindexed,
            &constraints,
            &LexicalPageSpec::first(10),
            &RequestBudgetV1::unbounded(),
        )?
        .candidates;
    assert_eq!(
        scanned
            .iter()
            .map(|candidate| candidate.candidate_id.as_str())
            .collect::<Vec<_>>(),
        vec!["requested"],
        "index:no must constrain during the scan, before result stabilization"
    );

    let language_error = searcher
        .search_constrained(
            &unindexed,
            &QueryConstraintSetV1::from_languages([language_code("rust")?]),
            &LexicalPageSpec::first(10),
            &RequestBudgetV1::unbounded(),
        )
        .expect_err("index:no must not infer typed language authority from a path extension");
    assert!(
        matches!(&language_error, CoreError::NotImplemented(message) if message.contains("typed language constraints require indexed execution")),
        "unexpected index:no typed-language error: {language_error:?}"
    );
    Ok(())
}

#[test]
fn exact_path_constraint_only_query_treats_dsl_metacharacters_as_literal_v1() -> TestResult {
    let dir = tempfile::tempdir()?;
    let adapter = LexicalAdapter::with_state_root(dir.path().to_path_buf());
    let literal_path = "src/a*)' \"literal file.rs";
    let ops = vec![
        upsert_with_metadata("literal-chunk", literal_path, "rust", 1, 1, "body")?,
        upsert_with_metadata("other-chunk", "src/other.rs", "rust", 1, 1, "body")?,
        upsert_symbol("literal-symbol", literal_path, "rust", "TargetSymbol", 1, 1)?,
        upsert_symbol("other-symbol", "src/other.rs", "rust", "TargetSymbol", 1, 1)?,
    ];
    adapter.build(&repo(), &revision(), generation(), &ops)?;
    let searcher = adapter.open(&repo(), &revision(), generation())?;
    let constraints = QueryConstraintSetV1::from_exact_repo_relative_path(
        ExactRepoRelativePathV1::new(literal_path).map_err(str::to_string)?,
    );
    let constraint_only = make_query(LqExpr::Empty);

    let symbols = searcher.search_symbols_constrained(
        &constraint_only,
        &constraints,
        &LexicalPageSpec::first(10),
        &RequestBudgetV1::unbounded(),
    )?;
    assert_eq!(
        symbols
            .candidates
            .iter()
            .map(|candidate| candidate.candidate_id.as_str())
            .collect::<Vec<_>>(),
        vec!["literal-symbol"],
        "typed path characters must be an exact term, never parsed as Sourcegraph syntax"
    );
    let chunks = searcher
        .search_constrained(
            &constraint_only,
            &constraints,
            &LexicalPageSpec::first(10),
            &RequestBudgetV1::unbounded(),
        )?
        .candidates;
    assert_eq!(
        chunks
            .iter()
            .map(|candidate| candidate.candidate_id.as_str())
            .collect::<Vec<_>>(),
        vec!["literal-chunk"]
    );

    for error in [
        searcher
            .search_symbols_constrained(
                &constraint_only,
                &QueryConstraintSetV1::unconstrained(),
                &LexicalPageSpec::first(10),
                &RequestBudgetV1::unbounded(),
            )
            .expect_err("empty unconstrained symbol query must remain rejected"),
        searcher
            .search_constrained(
                &constraint_only,
                &QueryConstraintSetV1::unconstrained(),
                &LexicalPageSpec::first(10),
                &RequestBudgetV1::unbounded(),
            )
            .expect_err("empty unconstrained text query must remain rejected"),
    ] {
        assert!(
            matches!(&error, CoreError::InvalidContract(message) if message.contains("empty query is rejected")),
            "unexpected empty-query rejection: {error:?}"
        );
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
        &RequestBudgetV1::unbounded(),
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
        &RequestBudgetV1::unbounded(),
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
        &RequestBudgetV1::unbounded(),
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
        &RequestBudgetV1::unbounded(),
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
        &RequestBudgetV1::unbounded(),
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
        &RequestBudgetV1::unbounded(),
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
    let sensitive_miss_hits =
        searcher.search(&sensitive_miss, 10, &RequestBudgetV1::unbounded())?;
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
    let sensitive_hit_hits = searcher.search(&sensitive_hit, 10, &RequestBudgetV1::unbounded())?;
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
        &RequestBudgetV1::unbounded(),
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
    let regexp_option_hits =
        searcher.search(&regexp_option_query, 10, &RequestBudgetV1::unbounded())?;
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
        &RequestBudgetV1::unbounded(),
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
        &RequestBudgetV1::unbounded(),
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
        &RequestBudgetV1::unbounded(),
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
fn symbol_content_authority_shapes_fail_closed_on_initial_and_replayed_batches() -> TestResult {
    use quanta_index_contract::SearchPlaneErrorCodeV2;
    for replayed in [false, true] {
        let dir = tempfile::tempdir()?;
        let adapter = LexicalAdapter::with_state_root(dir.path().to_path_buf());
        let ops = vec![
            upsert_with_metadata("chunk", "src/lib.rs", "rust", 1, 1, "content gate witness")?,
            upsert_symbol("sym", "src/symbol.rs", "rust", "needle_symbol", 2, 2)?,
        ];
        let batch = op_fixture::batch(&repo(), &revision(), generation(), &ops)?;
        if replayed {
            adapter.build_batch(&batch)?;
        }
        adapter.build_batch(&batch)?;
        let searcher = adapter.open(&repo(), &revision(), generation())?;
        let budget = RequestBudgetV1::unbounded();
        let keyword = make_query(LqExpr::Leaf(LqLeaf::Keyword("needle_symbol".to_string())));
        let mut content_keyword = keyword.clone();
        content_keyword.filters.push(LqFilter::Content {
            leaf: LqLeaf::Keyword("needle_symbol".to_string()),
        });
        for rows in [
            searcher.search_symbols(&content_keyword, 10, &budget)?,
            searcher.search_symbols_all(&content_keyword, &budget)?,
        ] {
            if rows.len() != 1 || rows.first().is_none_or(|row| row.candidate_id != "sym") {
                return Err(format!(
                    "replayed={replayed}: symbol keyword content filter missing: {rows:?}"
                )
                .into());
            }
        }
        for filter in [
            LqFilter::Type {
                kind: LqType::Symbol,
            },
            LqFilter::Select {
                dim: LqSelect::Symbol,
            },
        ] {
            let mut routed = content_keyword.clone();
            routed.filters.push(filter);
            let rows = searcher.search(&routed, 10, &budget)?;
            if rows.len() != 1 || rows.first().is_none_or(|row| row.candidate_id != "sym") {
                return Err(format!(
                    "replayed={replayed}: routed symbol keyword content filter missing: {rows:?}"
                )
                .into());
            }
        }
        for rows in [
            searcher.search_symbols(&keyword, 10, &budget)?,
            searcher.search_symbols_all(&keyword, &budget)?,
        ] {
            if rows.len() != 1 || rows.first().is_none_or(|row| row.candidate_id != "sym") {
                return Err(
                    format!("replayed={replayed}: keyword symbol missing: {rows:?}").into(),
                );
            }
        }
        let mut unsupported = Vec::new();
        for leaf in [
            LqLeaf::Phrase("needle_symbol".to_string()),
            LqLeaf::RawString("needle_symbol".to_string()),
            LqLeaf::Regex("needle_symbol".to_string()),
        ] {
            let expr = LqExpr::Leaf(leaf);
            unsupported.extend([
                make_query(expr.clone()),
                make_query(LqExpr::Not(Box::new(expr.clone()))),
                make_query(LqExpr::Any(vec![keyword.expr.clone(), expr.clone()])),
                make_query(LqExpr::All(vec![keyword.expr.clone(), expr])),
            ]);
        }
        for arg in [
            LqPredicateArg::Phrase("needle_symbol".to_string()),
            LqPredicateArg::RawString("needle_symbol".to_string()),
        ] {
            let expr = LqExpr::Leaf(LqLeaf::Predicate {
                name: "symbol.has.name".to_string(),
                args: vec![arg],
            });
            unsupported.extend([
                make_query(expr.clone()),
                make_query(LqExpr::All(vec![keyword.expr.clone(), expr])),
            ]);
        }
        let mut regexp = keyword.clone();
        regexp.options.pattern_type = LqPatternType::Regexp;
        unsupported.push(regexp);
        for leaf in [
            LqLeaf::Phrase("needle_symbol".to_string()),
            LqLeaf::RawString("needle_symbol".to_string()),
            LqLeaf::Regex("needle_symbol".to_string()),
        ] {
            let mut filtered = keyword.clone();
            filtered.filters.push(LqFilter::Content { leaf });
            unsupported.push(filtered.clone());
            filtered.expr = LqExpr::All(vec![
                keyword.expr.clone(),
                LqExpr::Leaf(LqLeaf::Predicate {
                    name: "repo.has.content".to_string(),
                    args: vec![LqPredicateArg::Keyword("absent_content_gate".to_string())],
                }),
            ]);
            unsupported.push(filtered);
        }
        unsupported.push(make_query(LqExpr::All(vec![
            LqExpr::Leaf(LqLeaf::Phrase("needle_symbol".to_string())),
            LqExpr::Leaf(LqLeaf::Predicate {
                name: "repo.has.content".to_string(),
                args: vec![LqPredicateArg::Keyword("absent_content_gate".to_string())],
            }),
        ])));
        for query in unsupported {
            let refusal = |error: CoreError| -> TestResult {
                if !matches!(
                    error,
                    CoreError::Typed {
                        code: SearchPlaneErrorCodeV2::LexPlannerUnsupportedFilterCombo,
                        ..
                    }
                ) {
                    return Err(format!("replayed={replayed}: wrong refusal {error:?}").into());
                }
                Ok(())
            };
            refusal(
                searcher
                    .search_symbols(&query, 10, &budget)
                    .err()
                    .ok_or("symbol paged query falsely succeeded")?,
            )?;
            refusal(
                searcher
                    .search_symbols_all(&query, &budget)
                    .err()
                    .ok_or("symbol all query falsely succeeded")?,
            )?;
            for filter in [
                LqFilter::Type {
                    kind: LqType::Symbol,
                },
                LqFilter::Select {
                    dim: LqSelect::Symbol,
                },
            ] {
                let mut routed = query.clone();
                routed.filters.push(filter);
                refusal(
                    searcher
                        .search(&routed, 10, &budget)
                        .err()
                        .ok_or("symbol-routed text query falsely succeeded")?,
                )?;
            }
        }
        // These predicates explicitly search chunks even when the result domain is symbols.
        for name in ["repo.has.content", "file.contains"] {
            let query = make_query(LqExpr::All(vec![
                keyword.expr.clone(),
                LqExpr::Leaf(LqLeaf::Predicate {
                    name: name.to_string(),
                    args: vec![LqPredicateArg::Phrase("content gate".to_string())],
                }),
            ]));
            let rows = searcher.search_symbols(&query, 10, &budget)?;
            if name == "repo.has.content"
                && (rows.len() != 1 || rows.first().is_none_or(|row| row.candidate_id != "sym"))
            {
                return Err(format!(
                    "replayed={replayed}: chunk content repo gate lost symbol keyword: {rows:?}"
                )
                .into());
            }
        }
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
        &RequestBudgetV1::unbounded(),
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
        &RequestBudgetV1::unbounded(),
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
        &RequestBudgetV1::unbounded(),
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
        &RequestBudgetV1::unbounded(),
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
        &RequestBudgetV1::unbounded(),
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
        &RequestBudgetV1::unbounded(),
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
        &RequestBudgetV1::unbounded(),
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
        &RequestBudgetV1::unbounded(),
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
        &RequestBudgetV1::unbounded(),
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
    let sensitive_hits = searcher.search(&sensitive_query, 10, &RequestBudgetV1::unbounded())?;
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
    let exact_case_hits = searcher.search(&exact_case_query, 10, &RequestBudgetV1::unbounded())?;
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
        // QI-BB-011: `path_only_needle` is one token of the shared normalizer
        // on the path surface (beta) and in content (eta, gamma); the bare
        // word `needle` no longer matches inside snake_case identifiers, so
        // the shared whole token is the keyword. Gamma's long line ranks
        // below eta's short one, which keeps the count bound truncating.
        upsert_with_metadata(
            "gamma",
            "scripts/helper.py",
            "python",
            1,
            1,
            "def alpha_content_needle(): pass  # the config path_only_needle key is read here",
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
        upsert_with_metadata(
            "eta",
            "src/bait.rs",
            "rust",
            1,
            1,
            "let needle_xx = 1; // path_only_needle",
        )?,
        upsert_symbol("theta", "src/sym.rs", "rust", "MyTypeSymbol", 1, 1)?,
    ];
    adapter.build(&repo(), &revision(), generation(), &ops)?;

    let searcher = adapter.open(&repo(), &revision(), generation())?;
    let mut query = make_query(LqExpr::Leaf(LqLeaf::Keyword(
        "path_only_needle".to_string(),
    )));
    query.options.count = Some(LqCountBound::Bounded(2));
    let hits = searcher.search(&query, 10, &RequestBudgetV1::unbounded())?;
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
fn tantivy_count_all_keeps_the_page_and_reports_the_exact_total() -> TestResult {
    // QI-BB-005: `count:all` no longer widens the page to the whole match
    // set. The rows stay bounded by the caller's `top_k`; what the option
    // buys is the exact total, computed by the count collector without
    // materializing a single extra document.
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
    let page = searcher.search_constrained(
        &query,
        &QueryConstraintSetV1::unconstrained(),
        &LexicalPageSpec::first(1),
        &RequestBudgetV1::unbounded(),
    )?;
    let ids = page
        .candidates
        .iter()
        .map(|hit| hit.candidate_id.as_str())
        .collect::<Vec<_>>();
    if ids.len() != 1 {
        return Err(format!("count:all must keep the requested page of 1, got {ids:?}").into());
    }
    if page.exact_total != Some(3) {
        return Err(format!(
            "count:all must report the exact total of 3, got {:?}",
            page.exact_total
        )
        .into());
    }

    // Without a count option the total is not proven and the page is
    // exactly the requested rows.
    query.options.count = None;
    let plain = searcher.search_constrained(
        &query,
        &QueryConstraintSetV1::unconstrained(),
        &LexicalPageSpec::first(2),
        &RequestBudgetV1::unbounded(),
    )?;
    if plain.candidates.len() != 2 || plain.exact_total.is_some() {
        return Err(format!(
            "plain page drifted: rows={} exact_total={:?}",
            plain.candidates.len(),
            plain.exact_total
        )
        .into());
    }

    // A bounded count caps the page at min(top_k, N) and still proves the total.
    query.options.count = Some(LqCountBound::Bounded(2));
    let bounded = searcher.search_constrained(
        &query,
        &QueryConstraintSetV1::unconstrained(),
        &LexicalPageSpec::first(5),
        &RequestBudgetV1::unbounded(),
    )?;
    if bounded.candidates.len() != 2 || bounded.exact_total != Some(3) {
        return Err(format!(
            "count:2 drifted: rows={} exact_total={:?}",
            bounded.candidates.len(),
            bounded.exact_total
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
        &RequestBudgetV1::unbounded(),
    )?;
    let hit_ids: Vec<&str> = hits.iter().map(|hit| hit.candidate_id.as_str()).collect();
    if hit_ids != vec!["alpha", "gamma"] {
        return Err(format!(
            "expected select:file to collapse to per-path representatives [alpha, gamma], got {hit_ids:?}"
        )
        .into());
    }

    // The first two chunk rows belong to the same file. Grouping must run
    // before the top-k cut so a two-file page still includes src/main.rs.
    let query = make_query_with_filters(
        LqExpr::Leaf(LqLeaf::Keyword("file_projection_needle".to_string())),
        vec![LqFilter::Select {
            dim: LqSelect::File,
        }],
    );
    let file_hits = searcher.search(&query, 2, &RequestBudgetV1::unbounded())?;
    let file_ids: Vec<&str> = file_hits
        .iter()
        .map(|hit| hit.candidate_id.as_str())
        .collect();
    if file_ids != vec!["alpha", "gamma"] {
        return Err(format!(
            "expected select:file top-2 to group before cutting the page, got {file_ids:?}"
        )
        .into());
    }

    let chunk_query = make_query_with_filters(
        LqExpr::Leaf(LqLeaf::Keyword("file_projection_needle".to_string())),
        Vec::new(),
    );
    let chunk_hits = searcher.search(&chunk_query, 2, &RequestBudgetV1::unbounded())?;
    let chunk_ids: Vec<&str> = chunk_hits
        .iter()
        .map(|hit| hit.candidate_id.as_str())
        .collect();
    if chunk_ids != vec!["alpha", "beta"] {
        return Err(format!(
            "expected ordinary top-2 to preserve chunk ranking, got {chunk_ids:?}"
        )
        .into());
    }

    Ok(())
}

#[test]
fn tantivy_select_file_fills_page_after_more_than_ten_duplicate_chunks() -> TestResult {
    let dir = tempfile::tempdir()?;
    let adapter = LexicalAdapter::with_state_root(dir.path().to_path_buf());
    let mut ops = Vec::new();
    for index in 0..12 {
        ops.push(upsert_with_metadata(
            &format!("frequent-{index:02}"),
            "src/frequent_test.go",
            "go",
            index * 10 + 1,
            index * 10 + 2,
            "declaration_needle declaration_needle declaration_needle",
        )?);
    }
    ops.push(upsert_with_metadata(
        "definition",
        "src/definition.go",
        "go",
        1,
        2,
        "declaration_needle",
    )?);
    adapter.build(&repo(), &revision(), generation(), &ops)?;

    let searcher = adapter.open(&repo(), &revision(), generation())?;
    let expr = LqExpr::Leaf(LqLeaf::Keyword("declaration_needle".to_string()));
    let chunk_page =
        searcher.search(&make_query(expr.clone()), 10, &RequestBudgetV1::unbounded())?;
    assert_eq!(chunk_page.len(), 10);
    assert!(
        chunk_page
            .iter()
            .all(|hit| hit.repo_relative_path.as_str() == "src/frequent_test.go")
    );

    let file_page = searcher.search(
        &make_query_with_filters(
            expr,
            vec![LqFilter::Select {
                dim: LqSelect::File,
            }],
        ),
        2,
        &RequestBudgetV1::unbounded(),
    )?;
    let paths: Vec<&str> = file_page
        .iter()
        .map(|hit| hit.repo_relative_path.as_str())
        .collect();
    assert_eq!(paths, ["src/frequent_test.go", "src/definition.go"]);
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
        &RequestBudgetV1::unbounded(),
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
        &RequestBudgetV1::unbounded(),
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
        &RequestBudgetV1::unbounded(),
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
        &RequestBudgetV1::unbounded(),
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
        &RequestBudgetV1::unbounded(),
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
        &RequestBudgetV1::unbounded(),
    )?;
    if !mismatched_hits.is_empty() {
        return Err(
            format!("expected context mismatch to return 0 hits, got {mismatched_hits:?}").into(),
        );
    }

    Ok(())
}

#[test]
#[expect(
    clippy::similar_names,
    reason = "corp_a_* / corp_b_* bindings mirror the two-repo (corp-a, corp-b) allow-list fixture naming"
)]
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
        &RequestBudgetV1::unbounded(),
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
        &RequestBudgetV1::unbounded(),
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
        &RequestBudgetV1::unbounded(),
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
        .search(&gate_query, 10, &RequestBudgetV1::unbounded())?
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
        .search(&scalar_gate_query, 10, &RequestBudgetV1::unbounded())?
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
        .search(&name_gate_query, 10, &RequestBudgetV1::unbounded())?
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
    let name_miss_hits = searcher.search(&name_miss_query, 10, &RequestBudgetV1::unbounded())?;
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
    let scalar_miss_hits =
        searcher.search(&scalar_miss_query, 10, &RequestBudgetV1::unbounded())?;
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
            .search(&combo_query, 10, &RequestBudgetV1::unbounded())?
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
        let combo_hits = searcher.search(&combo_query, 10, &RequestBudgetV1::unbounded())?;
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
        .search(&gate_query, 10, &RequestBudgetV1::unbounded())?
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
    let miss_hits = searcher.search(&miss_query, 10, &RequestBudgetV1::unbounded())?;
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
            .search(&gate_query, 10, &RequestBudgetV1::unbounded())?
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
        .search(&repo_path_alias, 10, &RequestBudgetV1::unbounded())?
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
        .search(&repo_content_alias, 10, &RequestBudgetV1::unbounded())?
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
        .search(&file_contains_alias, 10, &RequestBudgetV1::unbounded())?
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
            .search(&query, 10, &RequestBudgetV1::unbounded())?
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
    let file_has_content_miss_hits =
        searcher.search(&file_has_content_miss, 10, &RequestBudgetV1::unbounded())?;
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
        .search(&repo_number_gate, 10, &RequestBudgetV1::unbounded())?
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
    let gate_hits = searcher.search(&gate_query, 10, &RequestBudgetV1::unbounded())?;
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
    let miss_hits = searcher.search(&miss_query, 10, &RequestBudgetV1::unbounded())?;
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
        .search(&or_query, 10, &RequestBudgetV1::unbounded())?
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
    let mut manual_or = or_query;
    manual_or.options.index_mode = Some(LqYesNoOnly::No);
    let mut manual_or_ids: Vec<String> = searcher
        .search(&manual_or, 10, &RequestBudgetV1::unbounded())?
        .into_iter()
        .map(|candidate| candidate.candidate_id)
        .collect();
    manual_or_ids.sort();
    assert_eq!(manual_or_ids, or_ids, "index:no repo.has.content OR");

    let not_true_query = make_query(LqExpr::All(vec![
        LqExpr::Not(Box::new(LqExpr::Leaf(LqLeaf::Predicate {
            name: "repo.has.content".to_string(),
            args: vec![LqPredicateArg::Phrase("gate-a only".to_string())],
        }))),
        LqExpr::Leaf(LqLeaf::Keyword("shared_oracle_needle".to_string())),
    ]));
    let mut not_true_ids: Vec<String> = searcher
        .search(&not_true_query, 10, &RequestBudgetV1::unbounded())?
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
    let mut manual_not = not_true_query;
    manual_not.options.index_mode = Some(LqYesNoOnly::No);
    let manual_not_ids: Vec<String> = searcher
        .search(&manual_not, 10, &RequestBudgetV1::unbounded())?
        .into_iter()
        .map(|candidate| candidate.candidate_id)
        .collect();
    assert_eq!(
        manual_not_ids, not_true_ids,
        "index:no repo.has.content NOT"
    );

    // QI-BB-011: a keyword matches whole tokens of the shared normalizer, and
    // `shared_oracle_needle` is one token (`_` never splits), so the corpus
    // word itself is the keyword here; `needle` alone would match nothing.
    let not_false_query = make_query(LqExpr::All(vec![
        LqExpr::Not(Box::new(LqExpr::Leaf(LqLeaf::Predicate {
            name: "repo.has.content".to_string(),
            args: vec![LqPredicateArg::Keyword("missing-corpus-token".to_string())],
        }))),
        LqExpr::Leaf(LqLeaf::Keyword("shared_oracle_needle".to_string())),
    ]));
    let mut not_false_ids: Vec<String> = searcher
        .search(&not_false_query, 10, &RequestBudgetV1::unbounded())?
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
    let hits = searcher.search(&hit_query, 10, &RequestBudgetV1::unbounded())?;
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
    let miss = searcher.search(&miss_query, 10, &RequestBudgetV1::unbounded())?;
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
        .search(&or_query, 10, &RequestBudgetV1::unbounded())?
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
    let mut manual_or = or_query;
    manual_or.options.index_mode = Some(LqYesNoOnly::No);
    let mut manual_or_ids: Vec<String> = searcher
        .search(&manual_or, 10, &RequestBudgetV1::unbounded())?
        .into_iter()
        .map(|candidate| candidate.candidate_id)
        .collect();
    manual_or_ids.sort();
    assert_eq!(manual_or_ids, or_ids, "index:no repo.has.file OR");

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
        .search(&not_true_query, 10, &RequestBudgetV1::unbounded())?
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
    let mut manual_not = not_true_query;
    manual_not.options.index_mode = Some(LqYesNoOnly::No);
    let manual_not_ids: Vec<String> = searcher
        .search(&manual_not, 10, &RequestBudgetV1::unbounded())?
        .into_iter()
        .map(|candidate| candidate.candidate_id)
        .collect();
    assert_eq!(manual_not_ids, not_true_ids, "index:no repo.has.file NOT");

    // QI-BB-011: a keyword matches whole tokens of the shared normalizer, and
    // `shared_oracle_needle` is one token (`_` never splits), so the corpus
    // word itself is the keyword here; `needle` alone would match nothing.
    let not_false_query = make_query(LqExpr::All(vec![
        LqExpr::Not(Box::new(LqExpr::Leaf(LqLeaf::Predicate {
            name: "repo.has.file".to_string(),
            args: vec![LqPredicateArg::Filter {
                name: "path".to_string(),
                value: "missing.rs".to_string(),
            }],
        }))),
        LqExpr::Leaf(LqLeaf::Keyword("shared_oracle_needle".to_string())),
    ]));
    let mut not_false_ids: Vec<String> = searcher
        .search(&not_false_query, 10, &RequestBudgetV1::unbounded())?
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
        .search(&phrase_hit_query, 10, &RequestBudgetV1::unbounded())?
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
    let phrase_miss_hits =
        searcher.search(&phrase_miss_query, 10, &RequestBudgetV1::unbounded())?;
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
        .search(&regex_hit_query, 10, &RequestBudgetV1::unbounded())?
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
        .search(&contains_hit_query, 10, &RequestBudgetV1::unbounded())?
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
        .search(
            &contains_phrase_hit_query,
            10,
            &RequestBudgetV1::unbounded(),
        )?
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
    let contains_miss_hits =
        searcher.search(&contains_miss_query, 10, &RequestBudgetV1::unbounded())?;
    if !contains_miss_hits.is_empty() {
        return Err(format!(
            "expected file.contains reversed phrase predicate to miss, got {contains_miss_hits:?}"
        )
        .into());
    }

    Ok(())
}

#[test]
fn tantivy_executes_scoped_file_content_predicates_across_boolean_contexts() -> TestResult {
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
            .search(&query, 10, &RequestBudgetV1::unbounded())?
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
    let file_miss_hits = searcher.search(&file_miss_query, 10, &RequestBudgetV1::unbounded())?;
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
    let lang_miss_hits = searcher.search(&lang_miss_query, 10, &RequestBudgetV1::unbounded())?;
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
        .search(&and_query, 10, &RequestBudgetV1::unbounded())?
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
        .search(&and_miss_query, 10, &RequestBudgetV1::unbounded())?
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
    let mut or_ids: Vec<String> = searcher
        .search(&or_query, 10, &RequestBudgetV1::unbounded())?
        .into_iter()
        .map(|hit| hit.candidate_id)
        .collect();
    or_ids.sort();
    if or_ids != vec!["alpha".to_string(), "phrase_hit".to_string()] {
        return Err(format!(
            "expected scoped file.contains OR to hit [alpha, phrase_hit], got {or_ids:?}"
        )
        .into());
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
    let mut not_ids: Vec<String> = searcher
        .search(&not_query, 10, &RequestBudgetV1::unbounded())?
        .into_iter()
        .map(|hit| hit.candidate_id)
        .collect();
    not_ids.sort();
    if not_ids
        != vec![
            "alpha".to_string(),
            "phrase_miss".to_string(),
            "regex_hit".to_string(),
        ]
    {
        return Err(format!(
            "expected scoped file.contains NOT to exclude only phrase_hit, got {not_ids:?}"
        )
        .into());
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
    let matcher_ids: Vec<String> = searcher
        .search(&bad_matcher_query, 10, &RequestBudgetV1::unbounded())?
        .into_iter()
        .map(|hit| hit.candidate_id)
        .collect();
    if matcher_ids != vec!["phrase_hit".to_string()] {
        return Err(format!(
            "expected file.contains(name:...) to hit [phrase_hit], got {matcher_ids:?}"
        )
        .into());
    }

    let multiple_scalars_query = make_query(LqExpr::Leaf(LqLeaf::Predicate {
        name: "file.contains".to_string(),
        args: vec![
            LqPredicateArg::Phrase("lemon".to_string()),
            LqPredicateArg::Phrase("banana".to_string()),
        ],
    }));
    match searcher.search(&multiple_scalars_query, 10, &RequestBudgetV1::unbounded()) {
        Err(CoreError::Typed {
            code: quanta_index_contract::SearchPlaneErrorCodeV2::LexPredicateUnimplemented,
            ..
        }) => {}
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
    match searcher.search(&repo_scoped_query, 10, &RequestBudgetV1::unbounded()) {
        Err(CoreError::Typed {
            code: quanta_index_contract::SearchPlaneErrorCodeV2::LexPredicateUnimplemented,
            ..
        }) => {}
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
    let hits = searcher.search_all(
        &make_query(LqExpr::Leaf(LqLeaf::Keyword("scope".to_string()))),
        &RequestBudgetV1::unbounded(),
    )?;
    let mut ids: Vec<String> = hits.into_iter().map(|c| c.candidate_id).collect();
    ids.sort();
    if ids != vec!["c1".to_string(), "c2".to_string(), "c3".to_string()] {
        return Err(format!("expected full scope ids [c1, c2, c3], got {ids:?}").into());
    }
    Ok(())
}

#[test]
fn independent_chunk_clear_refuses_and_file_tombstone_preserves_symbols_v1() -> TestResult {
    let dir = tempfile::tempdir()?;
    let adapter = LexicalAdapter::with_state_root(dir.path().to_path_buf());
    let base_generation = generation();
    let base_ops = vec![
        upsert("clear-chunk", "clear surface needle")?,
        upsert_symbol("kept-symbol", "src/lib.rs", "rust", "KeptSymbol", 1, 1)?,
    ];
    adapter.build(&repo(), &revision(), base_generation, &base_ops)?;

    let target_generation = ManifestGeneration::new(2);
    let mut delta =
        source_fixture::sealed_batch(&repo(), &revision(), target_generation, Vec::new())?;
    delta.mode = BatchIngestMode::Delta;
    delta.base_generation = Some(base_generation);
    delta.source_event.expected_base_event_id = Some("event-1".into());
    delta.clear_surfaces = vec![SearchScopeSurface::Chunk];
    delta.source_event.payload_sha256 = quanta_index_contract::source_event_payload_sha256(&delta)?;
    match adapter.build_batch(&delta) {
        Err(CoreError::InvalidContract(message))
            if message.contains("forbid independent Chunk/Symbol clear") => {}
        other => return Err(format!("independent clear did not refuse: {other:?}").into()),
    }
    let target_dir =
        quanta_index_core::GenerationStorageKeyV1::for_repo_revision(&repo(), &revision())
            .generation_dir(dir.path(), target_generation);
    if target_dir.exists() {
        return Err("refused clear created target generation state".into());
    }
    // Whole-file deletion is the admitted operation for this text-only file.
    // The separate symbol file and the old immutable generation must survive.
    delta.clear_surfaces.clear();
    delta
        .tombstone_scopes
        .push(quanta_index_contract::SearchCorpusTombstoneScope {
            file: source_fixture::file_key(&repo(), "src/smoke.txt"),
        });
    delta.source_event.payload_sha256 = quanta_index_contract::source_event_payload_sha256(&delta)?;
    adapter.build_batch(&delta)?;

    let searcher = adapter.open(&repo(), &revision(), target_generation)?;
    let chunk_hits = searcher.search(
        &make_query(LqExpr::Leaf(LqLeaf::Keyword("needle".to_string()))),
        10,
        &RequestBudgetV1::unbounded(),
    )?;
    let symbol_hits = searcher.search_symbols(
        &make_query(LqExpr::Leaf(LqLeaf::Keyword("KeptSymbol".to_string()))),
        10,
        &RequestBudgetV1::unbounded(),
    )?;
    let base = adapter.open(&repo(), &revision(), base_generation)?;
    let original = base.search(
        &make_query(LqExpr::Leaf(LqLeaf::Keyword("needle".into()))),
        10,
        &RequestBudgetV1::unbounded(),
    )?;
    assert_eq!(original.len(), 1);
    assert!(chunk_hits.is_empty());
    assert_eq!(symbol_hits.len(), 1);
    let kept = symbol_hits
        .first()
        .ok_or("symbol hits empty after length check")?;
    assert_eq!(kept.candidate_id, "kept-symbol");
    Ok(())
}

#[test]
fn tantivy_executes_index_no_full_scan_with_scoped_content_predicate() -> TestResult {
    let dir = tempfile::tempdir()?;
    let adapter = LexicalAdapter::with_state_root(dir.path().to_path_buf());

    let ops = vec![
        upsert_with_metadata("alpha", "src/lib.rs", "rust", 1, 2, "needle alpha")?,
        upsert_with_metadata("beta", "src/main.rs", "rust", 1, 2, "needle beta")?,
        upsert_with_metadata("gamma", "docs/readme.md", "markdown", 1, 2, "needle gamma")?,
    ];
    adapter.build(&repo(), &revision(), generation(), &ops)?;

    let searcher = adapter.open(&repo(), &revision(), generation())?;
    for name_pattern in [r"lib\.rs", r"\blib\.rs\b"] {
        let scoped = LqExpr::Leaf(LqLeaf::Predicate {
            name: "file.contains".to_string(),
            args: vec![
                LqPredicateArg::Filter {
                    name: "name".to_string(),
                    value: name_pattern.to_string(),
                },
                LqPredicateArg::Keyword("needle".to_string()),
            ],
        });
        for expr in [
            scoped.clone(),
            LqExpr::All(vec![
                scoped.clone(),
                LqExpr::Leaf(LqLeaf::Keyword("needle".into())),
            ]),
            LqExpr::Any(vec![
                scoped.clone(),
                LqExpr::Leaf(LqLeaf::Keyword("absent".into())),
            ]),
        ] {
            let mut query = make_query(expr);
            query.options.index_mode = Some(LqYesNoOnly::No);

            let hits = searcher.search(&query, 10, &RequestBudgetV1::unbounded())?;
            let ids: Vec<String> = hits.into_iter().map(|hit| hit.candidate_id).collect();
            if ids != ["alpha".to_string()] {
                return Err(format!(
                    "expected index:no scoped content predicate {name_pattern:?} to match [alpha], got {ids:?}"
                )
                .into());
            }
        }
        let mut negated = make_query(LqExpr::Not(Box::new(scoped.clone())));
        negated.options.index_mode = Some(LqYesNoOnly::No);
        let negated_ids: std::collections::BTreeSet<String> = searcher
            .search(&negated, 10, &RequestBudgetV1::unbounded())?
            .into_iter()
            .map(|hit| hit.candidate_id)
            .collect();
        if negated_ids != ["beta".to_string(), "gamma".to_string()].into() {
            return Err(format!(
                "index:no negated scoped content predicate {name_pattern:?} drifted: {negated_ids:?}"
            )
            .into());
        }
        let LqExpr::Leaf(leaf) = scoped else {
            return Err("scoped predicate fixture lost its leaf shape".into());
        };
        let mut filtered = make_query(LqExpr::Empty);
        filtered.options.index_mode = Some(LqYesNoOnly::No);
        filtered.filters.push(LqFilter::Content { leaf });
        let filtered_ids: Vec<String> = searcher
            .search(&filtered, 10, &RequestBudgetV1::unbounded())?
            .into_iter()
            .map(|hit| hit.candidate_id)
            .collect();
        if filtered_ids != ["alpha".to_string()] {
            return Err(format!(
                "index:no content filter {name_pattern:?} drifted: {filtered_ids:?}"
            )
            .into());
        }
    }

    Ok(())
}

#[test]
fn tantivy_index_no_language_refuses_without_stored_authority() -> TestResult {
    let dir = tempfile::tempdir()?;
    let adapter = LexicalAdapter::with_state_root(dir.path().to_path_buf());
    let ops = vec![
        upsert_with_metadata("python-source", "src/explicit.rs", "python", 1, 2, "needle")?,
        upsert_with_metadata("rust-source", "src/explicit.py", "rust", 1, 2, "needle")?,
    ];
    adapter.build(&repo(), &revision(), generation(), &ops)?;
    let searcher = adapter.open(&repo(), &revision(), generation())?;

    let all_ids: Vec<String> = searcher
        .search(
            &make_query(LqExpr::Leaf(LqLeaf::Keyword("needle".to_string()))),
            10,
            &RequestBudgetV1::unbounded(),
        )?
        .into_iter()
        .map(|candidate| candidate.candidate_id)
        .collect();
    assert_eq!(
        all_ids.len(),
        2,
        "fixture did not publish both text documents: {all_ids:?}"
    );
    let mut unindexed = make_query(LqExpr::Leaf(LqLeaf::Keyword("needle".to_string())));
    unindexed.options.index_mode = Some(LqYesNoOnly::No);
    assert_eq!(
        searcher
            .search(&unindexed, 10, &RequestBudgetV1::unbounded())?
            .len(),
        2
    );

    for (language, expected) in [("python", "python-source"), ("rust", "rust-source")] {
        let base = make_query_with_filters(
            LqExpr::Leaf(LqLeaf::Keyword("needle".to_string())),
            vec![LqFilter::Lang {
                id: language.to_string(),
            }],
        );
        let ids: Vec<String> = searcher
            .search(&base, 10, &RequestBudgetV1::unbounded())?
            .into_iter()
            .map(|candidate| candidate.candidate_id)
            .collect();
        assert_eq!(ids, [expected.to_string()], "indexed language={language}");
        let mut unindexed = base;
        unindexed.options.index_mode = Some(LqYesNoOnly::No);
        let refusal = searcher
            .search(&unindexed, 10, &RequestBudgetV1::unbounded())
            .expect_err("index:no must refuse language filtering without stored authority");
        assert!(
            matches!(&refusal, CoreError::NotImplemented(message) if message.contains("language filtering requires indexed execution")),
            "{refusal:?}"
        );
    }

    let mut scoped = make_query(LqExpr::Leaf(LqLeaf::Predicate {
        name: "file.contains".to_string(),
        args: vec![
            LqPredicateArg::Filter {
                name: "lang".to_string(),
                value: "python".to_string(),
            },
            LqPredicateArg::Keyword("needle".to_string()),
        ],
    }));
    scoped.options.index_mode = Some(LqYesNoOnly::No);
    let refusal = searcher
        .search(&scoped, 10, &RequestBudgetV1::unbounded())
        .expect_err("index:no must refuse scoped content language without stored authority");
    assert!(
        matches!(&refusal, CoreError::NotImplemented(message) if message.contains("language filtering requires indexed execution")),
        "{refusal:?}"
    );
    let candidate_ids =
        std::iter::once("rust-source".to_string()).collect::<std::collections::BTreeSet<_>>();
    let admission_refusal = searcher
        .admitted_candidates(
            &scoped,
            &QueryConstraintSetV1::unconstrained(),
            &candidate_ids,
            &RequestBudgetV1::unbounded(),
        )
        .expect_err("dense manual admission must refuse unsupported language authority");
    assert!(
        matches!(&admission_refusal, CoreError::NotImplemented(message) if message.contains("language filtering requires indexed execution")),
        "{admission_refusal:?}"
    );
    let explanation_refusal = searcher
        .explain_candidate(
            &scoped,
            &QueryConstraintSetV1::unconstrained(),
            "rust-source",
            &RequestBudgetV1::unbounded(),
        )
        .expect_err("manual explanation must refuse unsupported language authority");
    assert!(
        matches!(&explanation_refusal, CoreError::NotImplemented(message) if message.contains("language filtering requires indexed execution")),
        "{explanation_refusal:?}"
    );
    let mut absent_scoped = make_query(LqExpr::Leaf(LqLeaf::Predicate {
        name: "file.contains".to_string(),
        args: vec![
            LqPredicateArg::Filter {
                name: "name".to_string(),
                value: "never-present".to_string(),
            },
            LqPredicateArg::Filter {
                name: "lang".to_string(),
                value: "python".to_string(),
            },
            LqPredicateArg::Keyword("needle".to_string()),
        ],
    }));
    absent_scoped.options.index_mode = Some(LqYesNoOnly::No);
    let refusal = searcher
        .search(&absent_scoped, 10, &RequestBudgetV1::unbounded())
        .expect_err("index:no must refuse unavailable language even for an empty path scope");
    assert!(
        matches!(&refusal, CoreError::NotImplemented(message) if message.contains("language filtering requires indexed execution")),
        "{refusal:?}"
    );
    Ok(())
}

#[test]
fn tantivy_applies_query_boost_to_scores() -> TestResult {
    let dir = tempfile::tempdir()?;
    let adapter = LexicalAdapter::with_state_root(dir.path().to_path_buf());

    let ops = vec![
        upsert("alpha", "boosted needle alpha")?,
        upsert("beta", "boosted miss")?,
    ];
    adapter.build(&repo(), &revision(), generation(), &ops)?;

    let searcher = adapter.open(&repo(), &revision(), generation())?;
    let baseline_query = make_query(LqExpr::Leaf(LqLeaf::Keyword("needle".to_string())));
    let baseline_hits = searcher.search(&baseline_query, 10, &RequestBudgetV1::unbounded())?;
    let baseline_first = baseline_hits
        .first()
        .ok_or("baseline boost test returned no hits")?;

    let mut boosted_query = baseline_query;
    boosted_query.options.boost_millis = Some(5_000);
    let boosted_hits = searcher.search(&boosted_query, 10, &RequestBudgetV1::unbounded())?;
    let boosted_first = boosted_hits
        .first()
        .ok_or("boosted query returned no hits")?;

    if boosted_hits
        .iter()
        .map(|hit| hit.candidate_id.clone())
        .collect::<Vec<_>>()
        != baseline_hits
            .iter()
            .map(|hit| hit.candidate_id.clone())
            .collect::<Vec<_>>()
    {
        return Err("boost changed ranked candidate ids".into());
    }
    if boosted_first.score <= baseline_first.score {
        return Err(format!(
            "expected boost to increase score magnitude, baseline={} boosted={}",
            baseline_first.score, boosted_first.score
        )
        .into());
    }

    Ok(())
}
