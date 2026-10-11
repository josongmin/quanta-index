//! QI-BB-016 — every open lexical writer lives under one heap envelope, and
//! a writer nobody needs gives its heap back.
//!
//! The public publication oracle is that sealing releases the writer and
//! leaves its committed rows readable. Internal LRU and idle policies are
//! verified against the private writer cache in its unit tests.

#![forbid(unsafe_code)]

#[path = "support/publication_fixture.rs"]
mod publication_fixture;
use publication_fixture::FixtureBatchBuildPort;

use quanta_index_contract::lex::LanguageCode;
use quanta_index_contract::{
    BatchIngestMode, ChunkId, ChunkRecord, LQ_VERSION_TAG, LqExpr, LqLeaf, LqOptions, LqQuery,
    LqSpan, ManifestGeneration, RepoId, RepoRelativePath, RevisionId, SearchCorpusIngestBatch,
};
use quanta_index_core::{
    LexicalExecutionBudgetV1, LexicalIndexOpenPort, LexicalWriterPolicy, RegexMatchCachePolicy,
    RequestBudgetV1,
};
use quanta_index_lexical::LexicalAdapter;
use quanta_index_lexical::regex::RegexPolicy;
use std::error::Error;

#[path = "support/current_source_fixture.rs"]
mod current_source_fixture;

type TestResult = Result<(), Box<dyn Error>>;

#[expect(
    clippy::expect_used,
    reason = "static fixture IDs provably satisfy the canonical ID policy"
)]
fn repo() -> RepoId {
    RepoId::new("envelope-repo").expect("static fixture ID satisfies canonical policy")
}

#[expect(
    clippy::expect_used,
    reason = "static fixture IDs provably satisfy the canonical ID policy"
)]
fn revision() -> RevisionId {
    RevisionId::new("envelope-rev").expect("static fixture ID satisfies canonical policy")
}

fn adapter(root: std::path::PathBuf, policy: LexicalWriterPolicy) -> LexicalAdapter {
    LexicalAdapter::with_state_root_and_policies(
        root,
        RegexPolicy::defaults(),
        LexicalExecutionBudgetV1::DEFAULT,
        RegexMatchCachePolicy::DEFAULT,
        policy,
    )
}

fn chunk(generation: u64) -> Result<ChunkRecord, Box<dyn Error>> {
    let text = format!("envelope marker generation {generation}");
    Ok(ChunkRecord {
        chunk_id: ChunkId::new(format!("chunk-g{generation}")),
        repo_relative_path: RepoRelativePath::new(format!("src/g{generation}.rs")),
        language: LanguageCode::new("text")
            .map_err(|err| -> Box<dyn Error> { format!("language: {err}").into() })?,
        start_byte: 0,
        end_byte: u32::try_from(text.len())?,
        start_line: 0,
        end_line: 0,
        text: text.into_boxed_str(),
        structural: None,
        parent_chunk_id: None,
        source_repo_id: None,
    })
}

fn marker_query() -> LqQuery {
    LqQuery {
        lq_version: LQ_VERSION_TAG,
        expr: LqExpr::Leaf(LqLeaf::Keyword("envelope".to_string())),
        filters: Vec::new(),
        options: LqOptions::defaults(),
        directives: Vec::new(),
        source_span: LqSpan::eof(0),
    }
}

fn hits(adapter: &LexicalAdapter, generation: u64) -> Result<usize, Box<dyn Error>> {
    let searcher = adapter.open(
        &repo(),
        &revision(),
        ManifestGeneration::new(generation),
        &quanta_index_core::RequestBudgetV1::unbounded(),
    )?;
    Ok(searcher
        .search(&marker_query(), 10, &RequestBudgetV1::unbounded())?
        .len())
}

#[test]
fn a_seal_releases_its_generations_writer_and_the_index_stays_readable() -> TestResult {
    let dir = tempfile::tempdir()?;
    let adapter = adapter(dir.path().to_path_buf(), LexicalWriterPolicy::DEFAULT);
    let generation = ManifestGeneration::new(7);
    let body = format!("envelope marker generation {}", generation.get());
    let replacement = current_source_fixture::text_scope(
        &repo(),
        &revision(),
        "src/g7.rs",
        LanguageCode::new("text")
            .map_err(|err| -> Box<dyn Error> { format!("language: {err}").into() })?,
        &body,
        vec![chunk(7)?],
    )?;
    let mut batch = SearchCorpusIngestBatch {
        source_event: current_source_fixture::empty_event(),
        repo_id: repo(),
        revision_id: revision(),
        generation,
        base_generation: None,
        manifest_digest: "manifest:7".to_string(),
        // Adapter admission checks token shape; IPC owns body-digest proof.
        batch_digest: "0".repeat(64),
        mode: BatchIngestMode::ReplaceGeneration,
        bundle_payload: None,
        clear_surfaces: Vec::new(),
        replace_scopes: vec![replacement],
        tombstone_scopes: Vec::new(),
        semantic_replace_scopes: Vec::new(),
        semantic_tombstone_scopes: Vec::new(),
        seal: true,
    };
    current_source_fixture::finish_batch(&mut batch)?;
    let _stages = adapter.build_fixture_batch(&batch)?;
    let stats = adapter.writer_cache_stats()?;
    if stats.open_writers != 0 || stats.seal_releases != 1 || stats.allocated_heap_bytes != 0 {
        return Err(format!("a sealed generation holds no writer: {stats:?}").into());
    }
    if hits(&adapter, 7)? != 1 {
        return Err("the sealed generation serves its row".into());
    }
    Ok(())
}
