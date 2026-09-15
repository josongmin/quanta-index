//! Delta generations must carry their base generation forward.
//!
//! A `Delta` search-corpus batch names a `base_generation`. Everything the base
//! held that the delta does not mutate has to remain queryable in the new
//! generation. These tests pin that invariant from the outside: they only ever
//! assert on what a searcher opened against the target generation returns, so
//! they stay valid across any change to how the base is physically carried
//! (full copy, hard link, native snapshot reuse).
//!
//! The second test covers the ordering hazard: several producer-facing
//! authority surfaces (`repo:has.commit.after(...)` recency, repo metadata,
//! topics, descriptions, file ownership, file contributors) are published per
//! generation and create the generation directory as a side effect. If one of
//! them lands before the lexical delta, the delta must still inherit the base.

#![forbid(unsafe_code)]

use std::error::Error;

use quanta_index_contract::lex::LanguageCode;
use quanta_index_contract::{
    BatchIngestMode, ChunkId, ChunkRecord, LQ_VERSION_TAG, LqExpr, LqLeaf, LqOptions, LqQuery,
    LqSpan, ManifestGeneration, RepoCommitRecencyEntry, RepoCommitRecencyIngestBatch, RepoId,
    RepoRelativePath, RevisionId, SearchCorpusIngestBatch, SearchCorpusReplaceScope,
    SearchScopeKey, SearchScopeSurface,
};
use quanta_index_core::{
    LexicalIndexOpenPort, RepoCommitRecencyIngestPort, SearchCorpusBatchBuildPort,
};
use quanta_index_lexical::LexicalAdapter;

type TestResult = Result<(), Box<dyn Error>>;

const ALPHA_PATH: &str = "src/alpha.rs";
const BETA_PATH: &str = "src/beta.rs";
const ALPHA_MARKER: &str = "alpha_marker";
const BETA_MARKER: &str = "beta_marker";
const BETA_MARKER_V2: &str = "gamma_replacement";

fn repo() -> RepoId {
    RepoId::new("carryforward-repo")
}

fn revision() -> RevisionId {
    RevisionId::new("carryforward-rev")
}

fn scope(
    path: &str,
    chunk_id: &str,
    body: &str,
) -> Result<SearchCorpusReplaceScope, Box<dyn Error>> {
    let language = LanguageCode::new("rust")
        .map_err(|err| -> Box<dyn Error> { format!("language code: {err}").into() })?;
    let end_byte = u32::try_from(body.len())
        .map_err(|err| -> Box<dyn Error> { format!("chunk body too large: {err}").into() })?;
    Ok(SearchCorpusReplaceScope {
        scope: SearchScopeKey {
            doc_surface: SearchScopeSurface::File,
            repo_relative_path: RepoRelativePath::new(path),
        },
        scope_digest: format!("scope:{path}:{chunk_id}"),
        chunks: vec![ChunkRecord {
            chunk_id: ChunkId::new(chunk_id),
            repo_relative_path: RepoRelativePath::new(path),
            language,
            start_byte: 0,
            end_byte,
            start_line: 1,
            end_line: 1,
            text: body.to_string().into_boxed_str(),
            structural: None,
            parent_chunk_id: None,
            source_repo_id: None,
        }],
        symbols: Vec::new(),
    })
}

fn base_batch(generation: ManifestGeneration) -> Result<SearchCorpusIngestBatch, Box<dyn Error>> {
    Ok(SearchCorpusIngestBatch {
        repo_id: repo(),
        revision_id: revision(),
        generation,
        base_generation: None,
        manifest_digest: format!("carryforward-manifest:{}", generation.get()),
        batch_digest: format!("carryforward-batch:{}", generation.get()),
        mode: BatchIngestMode::ReplaceGeneration,
        bundle_payload: None,
        clear_surfaces: Vec::new(),
        replace_scopes: vec![
            scope(ALPHA_PATH, "chunk-alpha", ALPHA_MARKER)?,
            scope(BETA_PATH, "chunk-beta", BETA_MARKER)?,
        ],
        tombstone_scopes: Vec::new(),
        semantic_replace_scopes: Vec::new(),
        semantic_tombstone_scopes: Vec::new(),
        seal: true,
    })
}

/// Replaces only `src/beta.rs`; `src/alpha.rs` must survive from the base.
fn delta_batch(
    generation: ManifestGeneration,
    base: ManifestGeneration,
) -> Result<SearchCorpusIngestBatch, Box<dyn Error>> {
    Ok(SearchCorpusIngestBatch {
        repo_id: repo(),
        revision_id: revision(),
        generation,
        base_generation: Some(base),
        manifest_digest: format!("carryforward-manifest:{}", generation.get()),
        batch_digest: format!("carryforward-batch:{}", generation.get()),
        mode: BatchIngestMode::Delta,
        bundle_payload: None,
        clear_surfaces: Vec::new(),
        replace_scopes: vec![scope(BETA_PATH, "chunk-beta", BETA_MARKER_V2)?],
        tombstone_scopes: Vec::new(),
        semantic_replace_scopes: Vec::new(),
        semantic_tombstone_scopes: Vec::new(),
        seal: true,
    })
}

fn keyword_query(term: &str) -> LqQuery {
    LqQuery {
        lq_version: LQ_VERSION_TAG,
        expr: LqExpr::Leaf(LqLeaf::Keyword(term.to_string())),
        filters: Vec::new(),
        options: LqOptions::defaults(),
        directives: Vec::new(),
        source_span: LqSpan::eof(0),
    }
}

fn hit_ids(
    adapter: &LexicalAdapter,
    generation: ManifestGeneration,
    term: &str,
) -> Result<Vec<String>, Box<dyn Error>> {
    let searcher = adapter.open(&repo(), &revision(), generation)?;
    let mut ids: Vec<String> = searcher
        .search(&keyword_query(term), 16)?
        .iter()
        .map(|candidate| candidate.candidate_id.clone())
        .collect();
    ids.sort();
    Ok(ids)
}

fn assert_hits(
    adapter: &LexicalAdapter,
    generation: ManifestGeneration,
    term: &str,
    expected: &[&str],
    label: &str,
) -> TestResult {
    let observed = hit_ids(adapter, generation, term)?;
    let expected_owned: Vec<String> = expected.iter().map(|id| (*id).to_string()).collect();
    if observed != expected_owned {
        return Err(format!(
            "{label}: generation g{} query `{term}` expected {expected_owned:?}, got {observed:?}",
            generation.get()
        )
        .into());
    }
    Ok(())
}

fn recency_batch(generation: ManifestGeneration) -> RepoCommitRecencyIngestBatch {
    RepoCommitRecencyIngestBatch {
        repo_id: repo(),
        revision_id: revision(),
        generation,
        batch_digest: format!("carryforward-recency:{}", generation.get()),
        entries: vec![RepoCommitRecencyEntry {
            source_repo_id: RepoId::new("source-repo"),
            latest_committer_time_ms: 1_700_000_000_000,
        }],
    }
}

/// Control: a delta with no prior generation-directory writer carries the base.
#[test]
fn delta_generation_inherits_unmutated_base_scopes() -> TestResult {
    let dir = tempfile::tempdir()?;
    let adapter = LexicalAdapter::with_state_root(dir.path().to_path_buf());
    let g1 = ManifestGeneration::new(1);
    let g2 = ManifestGeneration::new(2);

    adapter.build_batch(&base_batch(g1)?)?;
    assert_hits(&adapter, g1, ALPHA_MARKER, &["chunk-alpha"], "base")?;
    assert_hits(&adapter, g1, BETA_MARKER, &["chunk-beta"], "base")?;

    adapter.build_batch(&delta_batch(g2, g1)?)?;
    assert_hits(&adapter, g2, ALPHA_MARKER, &["chunk-alpha"], "delta")?;
    assert_hits(&adapter, g2, BETA_MARKER_V2, &["chunk-beta"], "delta")?;
    assert_hits(&adapter, g2, BETA_MARKER, &[], "delta")?;
    Ok(())
}

/// A sidecar authority published first must not cost the delta its base.
///
/// The recency authority creates the target generation directory before any
/// lexical op runs. The delta that follows still names `base_generation: g1`,
/// so `src/alpha.rs` has to remain queryable in g2.
#[test]
fn delta_generation_inherits_base_when_a_sidecar_authority_lands_first() -> TestResult {
    let dir = tempfile::tempdir()?;
    let adapter = LexicalAdapter::with_state_root(dir.path().to_path_buf());
    let g1 = ManifestGeneration::new(1);
    let g2 = ManifestGeneration::new(2);

    adapter.build_batch(&base_batch(g1)?)?;
    assert_hits(&adapter, g1, ALPHA_MARKER, &["chunk-alpha"], "base")?;

    let _receipt = adapter.publish_batch(&recency_batch(g2))?;
    adapter.build_batch(&delta_batch(g2, g1)?)?;

    assert_hits(
        &adapter,
        g2,
        ALPHA_MARKER,
        &["chunk-alpha"],
        "sidecar-first delta",
    )?;
    assert_hits(
        &adapter,
        g2,
        BETA_MARKER_V2,
        &["chunk-beta"],
        "sidecar-first delta",
    )?;
    Ok(())
}
