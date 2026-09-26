//! QI-BB-024 — a regex restricts the index as one compressed bitmap of
//! text-authority doc ids, the match cache is bounded by bytes and
//! cardinality, and a hit shares the set instead of copying it.
//!
//! Before this the cache was bounded by entry count alone, every entry was
//! the matched candidates' id strings, every hit cloned them, and the
//! restriction built one term query per match. Now a match set is a bitmap
//! shared by `Arc`, the restriction is one query over it, and the adapter's
//! cache runs under a `RegexMatchCachePolicy`: a result wider than one
//! entry may hold is served but not kept, an insert evicts least recently
//! used entries until the byte bound holds, and the stats say which.
//!
//! The oracles: an independent regex engine run over the fixture texts for
//! what a query must return; the adapter's stats over real sealed queries
//! for what was refused, evicted, built and resident.

#![forbid(unsafe_code)]

#[path = "support/source_fixture.rs"]
mod source_fixture;

use std::error::Error;

use quanta_index_contract::lex::LanguageCode;
use quanta_index_contract::{
    ChunkId, ChunkRecord, LQ_VERSION_TAG, LqExpr, LqLeaf, LqOptions, LqPatternType, LqQuery,
    LqSpan, ManifestGeneration, RepoId, RepoRelativePath, RevisionId, SearchCorpusIngestBatch,
    SearchCorpusReplaceScope,
};
use quanta_index_contract::{GenerationSnapshot, SearchPlaneTrackKind};
use quanta_index_core::{
    LexicalExecutionBudgetV1, LexicalIndexOpenPort, LexicalSearcher, LexicalWriterPolicy,
    RegexMatchCachePolicy, RegexMatchCacheStats, RequestBudgetV1, SealedGenerationReclaimOutcomeV1,
    SealedGenerationReclaimPort, SearchCorpusBatchBuildPort,
};
use quanta_index_lexical::LexicalAdapter;
use quanta_index_lexical::regex::RegexPolicy;

type TestResult = Result<(), Box<dyn Error>>;

const DOCS: u32 = 6;

#[expect(
    clippy::expect_used,
    reason = "static fixture IDs provably satisfy the canonical ID policy"
)]
fn repo() -> RepoId {
    RepoId::new("regex-cache-repo").expect("static fixture ID satisfies canonical policy")
}

#[expect(
    clippy::expect_used,
    reason = "static fixture IDs provably satisfy the canonical ID policy"
)]
fn revision() -> RevisionId {
    RevisionId::new("regex-cache-rev").expect("static fixture ID satisfies canonical policy")
}

/// Every doc carries the broad token; only doc `index` carries its own; a
/// third of them carry the tri token.
fn body(index: u32) -> String {
    let tri = if index.is_multiple_of(3) {
        " tri_token"
    } else {
        ""
    };
    format!("fn item_{index}() {{ broad_token narrow_{index}_token{tri} }}")
}

fn scope(index: u32) -> Result<SearchCorpusReplaceScope, Box<dyn Error>> {
    let path = format!("src/file_{index}.rs");
    let body = body(index);
    let language = LanguageCode::new("rust")
        .map_err(|err| -> Box<dyn Error> { format!("language code: {err}").into() })?;
    Ok(source_fixture::complete_file(
        source_fixture::file_key(&repo(), &path),
        &revision(),
        language.clone(),
        body.as_bytes(),
        vec![ChunkRecord {
            chunk_id: ChunkId::new(format!("chunk-{index}")),
            repo_relative_path: RepoRelativePath::new(&path),
            language,
            start_byte: 0,
            end_byte: u32::try_from(body.len())?,
            start_line: 1,
            end_line: 1,
            text: body.clone().into_boxed_str(),
            structural: None,
            parent_chunk_id: None,
            source_repo_id: None,
        }],
        Vec::new(),
    )?)
}

fn sealed_batch_of(docs: u32) -> Result<SearchCorpusIngestBatch, Box<dyn Error>> {
    let mut batch = source_fixture::sealed_batch(
        &repo(),
        &revision(),
        ManifestGeneration::new(1),
        (1..=docs).map(scope).collect::<Result<Vec<_>, _>>()?,
    )?;
    batch.manifest_digest = "regex-cache-manifest:1".into();
    Ok(batch)
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
    adapter_of(root, policy, DOCS)
}

fn adapter_of(
    root: std::path::PathBuf,
    policy: RegexMatchCachePolicy,
    docs: u32,
) -> Result<LexicalAdapter, Box<dyn Error>> {
    let adapter = LexicalAdapter::with_state_root_and_policies(
        root,
        RegexPolicy::defaults(),
        LexicalExecutionBudgetV1::DEFAULT,
        policy,
        LexicalWriterPolicy::DEFAULT,
    );
    adapter.build_batch(&sealed_batch_of(docs)?)?;
    Ok(adapter)
}

/// The candidates an independent regex engine finds in the fixture texts.
fn oracle(docs: u32, source: &str) -> Result<Vec<String>, Box<dyn Error>> {
    let pattern = regex::Regex::new(source)?;
    let mut ids: Vec<String> = (1..=docs)
        .filter(|index| pattern.is_match(&body(*index)))
        .map(|index| format!("chunk-{index}"))
        .collect();
    ids.sort();
    Ok(ids)
}

fn ids(searcher: &dyn LexicalSearcher, source: &str) -> Result<Vec<String>, Box<dyn Error>> {
    let mut ids: Vec<String> = searcher
        .search(&regex_query(source), 128, &RequestBudgetV1::unbounded())?
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
    if broad != oracle(DOCS, "broad_tok.n")? {
        return Err(format!("broad regex must match every doc, matched {broad:?}").into());
    }
    let after_broad = stats(&adapter)?;
    if after_broad.refused_cardinality != 1
        || after_broad.entries != 0
        || after_broad.sets_built != 1
        || after_broad.members_built != u64::from(DOCS)
        || after_broad.bytes_built == 0
    {
        return Err(format!(
            "broad result must be built once, refused and not resident: {after_broad:?}"
        )
        .into());
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
        || after_again.sets_built != after_first.sets_built
        || after_again.bytes_built != after_first.bytes_built
    {
        return Err(format!("a repeat must be a hit that builds nothing: {after_again:?}").into());
    }
    Ok(())
}

/// A regex restricts to exactly what an independent regex engine matches.
///
/// The oracle runs over the fixture texts, for sets from none to the whole
/// corpus (which walk a segment takes for which size is proven against the
/// stored ids in the crate's unit tests).
#[test]
fn a_regex_restricts_to_exactly_what_an_independent_engine_matches() -> TestResult {
    const CORPUS: u32 = 64;
    let temp = tempfile::tempdir()?;
    let adapter = adapter_of(
        temp.path().to_path_buf(),
        RegexMatchCachePolicy::DEFAULT,
        CORPUS,
    )?;
    let searcher = adapter.open(&repo(), &revision(), ManifestGeneration::new(1))?;
    for source in [
        "narrow_7_tok.n",
        "narrow_(1|2|3)_tok.n",
        "tri_tok.n",
        "broad_tok.n",
        "narrow_99_tok.n",
    ] {
        let expected = oracle(CORPUS, source)?;
        let served = ids(searcher.as_ref(), source)?;
        if served != expected {
            return Err(format!("{source}: served {served:?}, the oracle {expected:?}").into());
        }
        // The cached set answers the repeat identically.
        if ids(searcher.as_ref(), source)? != expected {
            return Err(format!("{source}: the hit answered differently").into());
        }
    }
    Ok(())
}

/// A query holding a searcher is not broken by the eviction of its match
/// set or by the reclaim of its generation: the searcher keeps what it
/// opened, and a set it recomputes after both answers as before.
#[test]
fn eviction_and_generation_reclaim_do_not_break_a_held_searcher() -> TestResult {
    let temp = tempfile::tempdir()?;
    let policy = RegexMatchCachePolicy::new(1, 1 << 20, 64)?;
    let adapter = adapter(temp.path().to_path_buf(), policy)?;
    let searcher = adapter.open(&repo(), &revision(), ManifestGeneration::new(1))?;
    let first = ids(searcher.as_ref(), "narrow_2_tok.n")?;
    if first != oracle(DOCS, "narrow_2_tok.n")? {
        return Err(format!("the first answer is the oracle's: {first:?}").into());
    }
    // One entry: the next regex evicts the first.
    let _other = ids(searcher.as_ref(), "narrow_5_tok.n")?;
    if stats(&adapter)?.evictions != 1 {
        return Err(format!("the first set was evicted: {:?}", stats(&adapter)?).into());
    }
    let reclaimed = adapter.reclaim_sealed_generation(&GenerationSnapshot {
        repo_id: repo(),
        revision_id: revision(),
        track: SearchPlaneTrackKind::Lexical,
        manifest_generation: ManifestGeneration::new(1),
        manifest_digest: "regex-cache-manifest:1".to_string(),
    })?;
    if !matches!(
        reclaimed,
        SealedGenerationReclaimOutcomeV1::Reclaimed { .. }
    ) {
        return Err(format!("the generation is reclaimed: {reclaimed:?}").into());
    }
    if stats(&adapter)?.entries != 0 {
        return Err("the reclaim invalidated the generation's cached sets".into());
    }
    if ids(searcher.as_ref(), "narrow_2_tok.n")? != first {
        return Err("the held searcher answers as before after eviction and reclaim".into());
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
