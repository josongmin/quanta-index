//! QI-BB-024 — the regex match cache is bounded by bytes and cardinality,
//! and a hit shares the match set instead of copying it.
//!
//! Before this the cache was bounded by entry count alone, so 128 broad
//! regexes over a large corpus could hold 128 copies of the corpus's
//! candidate ids, and every hit cloned its whole set. Now the adapter's
//! cache runs under a `RegexMatchCachePolicy`: a result wider than one
//! entry may hold is served but not kept, an insert evicts least recently
//! used entries until the byte bound holds, and the stats say which. The
//! oracles are the adapter's own stats over real sealed queries: what was
//! refused, what was evicted, how many bytes are resident, and that a
//! repeated query is a hit whose answer is unchanged.

#![forbid(unsafe_code)]

use std::error::Error;

use quanta_index_contract::lex::LanguageCode;
use quanta_index_contract::{
    BatchIngestMode, ChunkId, ChunkRecord, LQ_VERSION_TAG, LqExpr, LqLeaf, LqOptions,
    LqPatternType, LqQuery, LqSpan, ManifestGeneration, RepoId, RepoRelativePath, RevisionId,
    SearchCorpusIngestBatch, SearchCorpusReplaceScope, SearchScopeKey, SearchScopeSurface,
};
use quanta_index_core::{
    LexicalExecutionBudgetV1, LexicalIndexOpenPort, LexicalSearcher, RegexMatchCachePolicy,
    RegexMatchCacheStats, SearchCorpusBatchBuildPort,
};
use quanta_index_lexical::LexicalAdapter;
use quanta_index_lexical::regex::RegexPolicy;

type TestResult = Result<(), Box<dyn Error>>;

const DOCS: u32 = 6;

fn repo() -> RepoId {
    RepoId::new("regex-cache-repo")
}

fn revision() -> RevisionId {
    RevisionId::new("regex-cache-rev")
}

fn scope(index: u32) -> Result<SearchCorpusReplaceScope, Box<dyn Error>> {
    let path = format!("src/file_{index}.rs");
    // Every doc carries the broad token; only doc `index` carries its own.
    let body = format!("fn item_{index}() {{ broad_token narrow_{index}_token }}");
    let language = LanguageCode::new("rust")
        .map_err(|err| -> Box<dyn Error> { format!("language code: {err}").into() })?;
    Ok(SearchCorpusReplaceScope {
        scope: SearchScopeKey {
            doc_surface: SearchScopeSurface::File,
            repo_relative_path: RepoRelativePath::new(&path),
        },
        scope_digest: format!("scope:{path}"),
        chunks: vec![ChunkRecord {
            chunk_id: ChunkId::new(format!("chunk-{index}")),
            repo_relative_path: RepoRelativePath::new(&path),
            language,
            start_byte: 0,
            end_byte: u32::try_from(body.len())?,
            start_line: 1,
            end_line: 1,
            text: body.into_boxed_str(),
            structural: None,
            parent_chunk_id: None,
            source_repo_id: None,
        }],
        symbols: Vec::new(),
    })
}

fn sealed_batch() -> Result<SearchCorpusIngestBatch, Box<dyn Error>> {
    Ok(SearchCorpusIngestBatch {
        repo_id: repo(),
        revision_id: revision(),
        generation: ManifestGeneration::new(1),
        base_generation: None,
        manifest_digest: "regex-cache-manifest:1".to_string(),
        batch_digest: "regex-cache-batch:1".to_string(),
        mode: BatchIngestMode::ReplaceGeneration,
        bundle_payload: None,
        clear_surfaces: Vec::new(),
        replace_scopes: (1..=DOCS).map(scope).collect::<Result<Vec<_>, _>>()?,
        tombstone_scopes: Vec::new(),
        semantic_replace_scopes: Vec::new(),
        semantic_tombstone_scopes: Vec::new(),
        seal: true,
    })
}

fn regex_query(source: &str) -> LqQuery {
    let mut options = LqOptions::defaults();
    options.pattern_type = LqPatternType::Regexp;
    LqQuery {
        lq_version: LQ_VERSION_TAG,
        expr: LqExpr::Leaf(LqLeaf::Regex(source.to_string())),
        filters: Vec::new(),
        options,
        directives: Vec::new(),
        source_span: LqSpan::eof(0),
    }
}

fn adapter(
    root: std::path::PathBuf,
    policy: RegexMatchCachePolicy,
) -> Result<LexicalAdapter, Box<dyn Error>> {
    let adapter = LexicalAdapter::with_state_root_and_policies(
        root,
        RegexPolicy::defaults(),
        LexicalExecutionBudgetV1::DEFAULT,
        policy,
    );
    adapter.build_batch(&sealed_batch()?)?;
    Ok(adapter)
}

fn ids(searcher: &dyn LexicalSearcher, source: &str) -> Result<Vec<String>, Box<dyn Error>> {
    let mut ids: Vec<String> = searcher
        .search(&regex_query(source), 32)?
        .iter()
        .map(|candidate| candidate.candidate_id.clone())
        .collect();
    ids.sort();
    Ok(ids)
}

fn stats(adapter: &LexicalAdapter) -> Result<RegexMatchCacheStats, Box<dyn Error>> {
    Ok(adapter.regex_match_cache_stats()?)
}

/// A broad regex is served but not cached; a narrow one is a shared hit.
///
/// A regex that matches more candidates than one entry may hold is served
/// correctly and not cached; a narrow one is cached, and its repeat is a
/// hit with the same answer and no growth.
#[test]
fn a_broad_regex_is_served_but_not_cached_and_a_narrow_one_is_a_shared_hit() -> TestResult {
    let temp = tempfile::tempdir()?;
    let policy = RegexMatchCachePolicy::new(8, 1 << 20, 2)?;
    let adapter = adapter(temp.path().to_path_buf(), policy)?;
    let searcher = adapter.open(&repo(), &revision(), ManifestGeneration::new(1))?;

    let broad = ids(searcher.as_ref(), "broad_tok.n")?;
    if broad.len() != usize::try_from(DOCS)? {
        return Err(format!("broad regex must match every doc, matched {}", broad.len()).into());
    }
    let after_broad = stats(&adapter)?;
    if after_broad.refused_cardinality != 1 || after_broad.entries != 0 {
        return Err(
            format!("broad result must be refused and not resident: {after_broad:?}").into(),
        );
    }

    let narrow_first = ids(searcher.as_ref(), "narrow_3_tok.n")?;
    if narrow_first != vec!["chunk-3".to_string()] {
        return Err(format!("narrow regex must match its one doc, got {narrow_first:?}").into());
    }
    let after_first = stats(&adapter)?;
    if after_first.entries != 1 || after_first.hits != 0 || after_first.resident_bytes == 0 {
        return Err(format!("narrow result must be cached once: {after_first:?}").into());
    }
    let narrow_again = ids(searcher.as_ref(), "narrow_3_tok.n")?;
    if narrow_again != narrow_first {
        return Err("a hit must answer exactly as the miss did".into());
    }
    let after_again = stats(&adapter)?;
    if after_again.hits != 1
        || after_again.entries != 1
        || after_again.resident_bytes != after_first.resident_bytes
    {
        return Err(format!("a repeat must be a hit without growth: {after_again:?}").into());
    }
    Ok(())
}

/// Resident bytes never exceed the policy.
///
/// Under a tight byte bound, many distinct narrow regexes never push the
/// resident bytes past the policy: older entries are evicted and counted.
#[test]
fn resident_bytes_never_exceed_the_policy_under_many_distinct_regexes() -> TestResult {
    let temp = tempfile::tempdir()?;
    let probe = adapter(temp.path().join("probe"), RegexMatchCachePolicy::DEFAULT)?;
    let probe_searcher = probe.open(&repo(), &revision(), ManifestGeneration::new(1))?;
    let _one = ids(probe_searcher.as_ref(), "narrow_1_tok.n")?;
    let one_entry = stats(&probe)?.resident_bytes;
    if one_entry == 0 {
        return Err("one cached entry must account for its bytes".into());
    }

    // Room for two entries and no more.
    let policy = RegexMatchCachePolicy::new(64, one_entry.saturating_mul(2), 8)?;
    let adapter = adapter(temp.path().join("bounded"), policy)?;
    let searcher = adapter.open(&repo(), &revision(), ManifestGeneration::new(1))?;
    for index in 1..=DOCS {
        let expected = vec![format!("chunk-{index}")];
        if ids(searcher.as_ref(), &format!("narrow_{index}_tok.n"))? != expected {
            return Err(format!("narrow_{index} must match its doc").into());
        }
        let now = stats(&adapter)?;
        if now.resident_bytes > policy.max_resident_bytes() {
            return Err(format!(
                "resident bytes {} exceed the policy {} after {index} regexes",
                now.resident_bytes,
                policy.max_resident_bytes()
            )
            .into());
        }
    }
    let end = stats(&adapter)?;
    if end.entries != 2 || end.evictions != u64::from(DOCS).saturating_sub(2) {
        return Err(format!("expected two resident entries and the rest evicted: {end:?}").into());
    }
    // The evicted ones still answer correctly, as misses.
    if ids(searcher.as_ref(), "narrow_1_tok.n")? != vec!["chunk-1".to_string()] {
        return Err("an evicted regex must be recomputed correctly".into());
    }
    if stats(&adapter)?.misses != end.misses.saturating_add(1) {
        return Err("a recompute after eviction is a miss".into());
    }
    Ok(())
}
