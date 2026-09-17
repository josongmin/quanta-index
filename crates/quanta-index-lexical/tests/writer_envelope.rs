//! QI-BB-016 — every open lexical writer lives under one heap envelope, and
//! a writer nobody needs gives its heap back.
//!
//! The envelope is the policy's bound; the oracles are the adapter's own
//! accounting read after every step (never above the envelope, never more
//! writers than it holds), the on-disk index of every released generation
//! (its committed segment survives the writer's release), and the seal,
//! after which its generation's writer is gone.

#![forbid(unsafe_code)]

use std::error::Error;
use std::path::Path;
use std::time::Duration;

use quanta_index_contract::channel::LexicalChannelOp;
use quanta_index_contract::lex::LanguageCode;
use quanta_index_contract::{
    BatchIngestMode, ChunkId, ChunkRecord, LQ_VERSION_TAG, LqExpr, LqLeaf, LqOptions, LqQuery,
    LqSpan, ManifestGeneration, RepoId, RepoRelativePath, RevisionId, SearchCorpusIngestBatch,
    SearchCorpusReplaceScope, SearchScopeKey, SearchScopeSurface, UpsertChunk,
};
use quanta_index_core::{
    GenerationStorageKeyV1, LEXICAL_WRITER_HEAP_BYTES_MIN, LexicalExecutionBudgetV1,
    LexicalIndexBuildPort, LexicalIndexOpenPort, LexicalWriterPolicy, RegexMatchCachePolicy,
    SearchCorpusBatchBuildPort,
};
use quanta_index_lexical::LexicalAdapter;
use quanta_index_lexical::regex::RegexPolicy;

type TestResult = Result<(), Box<dyn Error>>;

fn repo() -> RepoId {
    RepoId::new("envelope-repo")
}

fn revision() -> RevisionId {
    RevisionId::new("envelope-rev")
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

fn upsert(generation: u64) -> Result<LexicalChannelOp, Box<dyn Error>> {
    let record = chunk(generation)?;
    let mut payload = Vec::new();
    ciborium::into_writer(&record, &mut payload)?;
    Ok(LexicalChannelOp::UpsertChunk(UpsertChunk {
        repo_id: repo(),
        revision_id: revision(),
        generation: ManifestGeneration::new(generation),
        chunk_id: ChunkId::new(format!("chunk-g{generation}")),
        payload,
    }))
}

/// Open (or touch) generation `generation`'s writer through the legacy
/// build port, which never seals.
fn build(adapter: &LexicalAdapter, generation: u64) -> TestResult {
    adapter.build(
        &repo(),
        &revision(),
        ManifestGeneration::new(generation),
        &[upsert(generation)?],
    )?;
    Ok(())
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
    let searcher = adapter.open(&repo(), &revision(), ManifestGeneration::new(generation))?;
    Ok(searcher.search(&marker_query(), 10)?.len())
}

/// Whether the generation's on-disk index carries a committed segment: an
/// unsealed generation cannot be opened through the port, so what a release
/// leaves behind is proven on the index metadata itself.
fn has_committed_segment(root: &Path, generation: u64) -> Result<bool, Box<dyn Error>> {
    let dir = GenerationStorageKeyV1::for_repo_revision(&repo(), &revision())
        .generation_dir(root, ManifestGeneration::new(generation));
    let meta = std::fs::read_to_string(dir.join("meta.json"))?;
    Ok(meta.contains("max_doc"))
}

#[test]
fn the_envelope_bounds_open_writers_and_their_heap_and_releases_the_oldest_committed() -> TestResult
{
    let dir = tempfile::tempdir()?;
    // Three writers of the minimum heap fit; the fourth must push one out.
    let policy = LexicalWriterPolicy::new(
        3 * LEXICAL_WRITER_HEAP_BYTES_MIN,
        LEXICAL_WRITER_HEAP_BYTES_MIN,
        Duration::from_secs(3600),
    )?;
    let adapter = adapter(dir.path().to_path_buf(), policy);
    if policy.max_writers() != 3 {
        return Err(format!(
            "the envelope holds three writers, not {}",
            policy.max_writers()
        )
        .into());
    }
    for generation in 0..5 {
        build(&adapter, generation)?;
        let stats = adapter.writer_cache_stats()?;
        if stats.open_writers > stats.max_writers
            || stats.allocated_heap_bytes > policy.envelope_bytes()
            || stats.max_writers != 3
        {
            return Err(
                format!("the envelope must hold after generation {generation}: {stats:?}").into(),
            );
        }
    }
    let stats = adapter.writer_cache_stats()?;
    if stats.open_writers != 3 || stats.lru_releases != 2 || stats.idle_releases != 0 {
        return Err(
            format!("five builds through a three-writer envelope release two: {stats:?}").into(),
        );
    }
    // A released generation keeps its committed rows; dropping the writer
    // rolls nothing back.
    for generation in 0..2 {
        if !has_committed_segment(dir.path(), generation)? {
            return Err(format!("generation {generation} must keep its rows after release").into());
        }
    }
    // Touching a released generation reopens it and releases the oldest
    // resident one (generation 2), never a newer one.
    build(&adapter, 0)?;
    let stats = adapter.writer_cache_stats()?;
    if stats.open_writers != 3 || stats.lru_releases != 3 {
        return Err(format!("reopening releases the least recently used: {stats:?}").into());
    }
    if !has_committed_segment(dir.path(), 2)? {
        return Err("generation 2 keeps its rows after release".into());
    }
    Ok(())
}

#[test]
fn an_idle_writer_is_committed_and_released_on_the_next_sweep() -> TestResult {
    let dir = tempfile::tempdir()?;
    // An idle interval a build cannot exceed on any sane host, and sleeps
    // comfortably past it; a build that happens to cross it only moves a
    // release earlier, which the accounting below still admits.
    let idle = Duration::from_millis(400);
    let policy = LexicalWriterPolicy::new(
        4 * LEXICAL_WRITER_HEAP_BYTES_MIN,
        LEXICAL_WRITER_HEAP_BYTES_MIN,
        idle,
    )?;
    let adapter = adapter(dir.path().to_path_buf(), policy);
    build(&adapter, 1)?;
    build(&adapter, 2)?;
    let stats = adapter.writer_cache_stats()?;
    if stats
        .open_writers
        .saturating_add(usize::try_from(stats.idle_releases)?)
        != 2
    {
        return Err(format!("two writers were opened: {stats:?}").into());
    }
    std::thread::sleep(idle.saturating_mul(2));
    adapter.release_idle_writers()?;
    let stats = adapter.writer_cache_stats()?;
    if stats.open_writers != 0 || stats.idle_releases != 2 || stats.allocated_heap_bytes != 0 {
        return Err(format!("both idle writers give their heap back: {stats:?}").into());
    }
    for generation in [1, 2] {
        if !has_committed_segment(dir.path(), generation)? {
            return Err(format!("generation {generation} keeps its rows after release").into());
        }
    }
    // The sweep at the next build releases the idle writer and keeps the
    // one that build just touched.
    build(&adapter, 3)?;
    std::thread::sleep(idle.saturating_mul(2));
    build(&adapter, 4)?;
    let stats = adapter.writer_cache_stats()?;
    if stats.open_writers != 1 || stats.idle_releases != 3 {
        return Err(
            format!("the build-time sweep releases only the idle writer: {stats:?}").into(),
        );
    }
    Ok(())
}

#[test]
fn a_seal_releases_its_generations_writer_and_the_index_stays_readable() -> TestResult {
    let dir = tempfile::tempdir()?;
    let adapter = adapter(dir.path().to_path_buf(), LexicalWriterPolicy::DEFAULT);
    let generation = ManifestGeneration::new(7);
    adapter.build_batch(&SearchCorpusIngestBatch {
        repo_id: repo(),
        revision_id: revision(),
        generation,
        base_generation: None,
        manifest_digest: "manifest:7".to_string(),
        batch_digest: "batch:7".to_string(),
        mode: BatchIngestMode::ReplaceGeneration,
        bundle_payload: None,
        clear_surfaces: Vec::new(),
        replace_scopes: vec![SearchCorpusReplaceScope {
            scope: SearchScopeKey {
                doc_surface: SearchScopeSurface::Chunk,
                repo_relative_path: RepoRelativePath::new("src/g7.rs"),
            },
            scope_digest: "scope:7".to_string(),
            chunks: vec![chunk(7)?],
            symbols: Vec::new(),
        }],
        tombstone_scopes: Vec::new(),
        semantic_replace_scopes: Vec::new(),
        semantic_tombstone_scopes: Vec::new(),
        seal: true,
    })?;
    let stats = adapter.writer_cache_stats()?;
    if stats.open_writers != 0 || stats.seal_releases != 1 || stats.allocated_heap_bytes != 0 {
        return Err(format!("a sealed generation holds no writer: {stats:?}").into());
    }
    if hits(&adapter, 7)? != 1 {
        return Err("the sealed generation serves its row".into());
    }
    Ok(())
}
