//! Smoke test for the Tantivy-backed lexical adapter.
//!
//! Builds an index from a sealed batch of `UpsertChunk` ops, opens a searcher,
//! and verifies BM25 scoring + boolean composition both return the expected
//! candidate sets. Follows the `wal_roundtrip.rs` test idiom: returns
//! `Result<(), Box<dyn Error>>` and propagates errors via `?` (no `.unwrap()`
//! or `.expect()` per the workspace lint policy).

#![forbid(unsafe_code)]

use std::error::Error;

use quanta_index_contract::{
    ChunkId, ChunkRecord, LQ_VERSION_TAG, LexicalChannelOp, LexicalFullBundle,
    LexicalRepoMetadataRecord, LqExpr, LqFileScope, LqFilter, LqLeaf, LqOptions, LqPatternType,
    LqPredicateArg, LqQuery, LqSpan, LqVisibility, LqYesNoOnly, ManifestGeneration, RepoId,
    RepoRelativePath, RevisionId, UpsertChunk,
};
use quanta_index_core::{LexicalIndexBuildPort, LexicalIndexOpenPort};
use quanta_index_lexical::{LEXICAL_WRITER_CACHE_MAX, LexicalAdapter};

type TestResult = Result<(), Box<dyn Error>>;

fn repo() -> RepoId {
    RepoId::new("smoke-repo")
}

fn revision() -> RevisionId {
    RevisionId::new("smoke-rev")
}

fn generation() -> ManifestGeneration {
    ManifestGeneration::new(1)
}

fn encode_chunk_payload(text: &str) -> Result<Vec<u8>, Box<dyn Error>> {
    encode_chunk_payload_with_metadata("", "", 0, 0, text)
}

fn encode_chunk_payload_with_metadata(
    repo_relative_path: &str,
    language: &str,
    start_line: u32,
    end_line: u32,
    text: &str,
) -> Result<Vec<u8>, Box<dyn Error>> {
    let record = ChunkRecord {
        repo_relative_path: RepoRelativePath::new(repo_relative_path),
        language: language.to_string().into_boxed_str(),
        start_line,
        end_line,
        snippet: text.to_string().into_boxed_str(),
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
    let record = LexicalRepoMetadataRecord {
        fork,
        archived,
        visibility,
        contexts: contexts.iter().map(ToString::to_string).collect(),
    };
    let mut payload = Vec::new();
    ciborium::into_writer(&record, &mut payload)
        .map_err(|err| -> Box<dyn Error> { format!("encode repo metadata: {err}").into() })?;
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
            repo_relative_path,
            language,
            start_line,
            end_line,
            text,
        )?,
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
            payload: encode_chunk_payload(&text)?,
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
