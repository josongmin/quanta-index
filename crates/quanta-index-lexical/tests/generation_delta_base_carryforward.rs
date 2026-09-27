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
//!
//! The third test pins the cost side of the same contract (QI-BB-006): a delta
//! may not rewrite the bytes it did not change. It asserts on *newly written
//! bytes*, computed by excluding storage the target shares with its base, so it
//! measures the property rather than the mechanism that achieves it.

#![forbid(unsafe_code)]

use std::collections::BTreeSet;
use std::error::Error;
use std::fmt::Write as _;
use std::os::unix::fs::MetadataExt as _;
use std::path::Path;

use sha2::{Digest as _, Sha256};

use quanta_index_contract::lex::LanguageCode;
use quanta_index_contract::{
    BatchIngestMode, ChunkId, ChunkRecord, LQ_VERSION_TAG, LqExpr, LqLeaf, LqOptions, LqQuery,
    LqSpan, ManifestGeneration, RepoCommitRecencyEntry, RepoCommitRecencyIngestBatch, RepoId,
    RepoRelativePath, RevisionId, SearchCorpusIngestBatch, SearchCorpusReplaceScope,
};
use quanta_index_core::{
    LexicalIndexOpenPort, RepoCommitRecencyIngestPort, RequestBudgetV1, SearchCorpusBatchBuildPort,
    TextAuthorityUpdateStats,
};
use quanta_index_lexical::LexicalAdapter;

#[path = "support/current_source_fixture.rs"]
mod current_source_fixture;

type TestResult = Result<(), Box<dyn Error>>;

const ALPHA_PATH: &str = "src/alpha.rs";
const BETA_PATH: &str = "src/beta.rs";
const ALPHA_MARKER: &str = "alpha_marker";
const BETA_MARKER: &str = "beta_marker";
/// Only present in the base body; a delta that replaces the scope must retire it.
const BETA_RETIRED_WORD: &str = "retiredsentinel";
/// Only present in the replacement body.
const BETA_FRESH_WORD: &str = "freshsentinel";
const BETA_MARKER_V2: &str = "gamma_replacement";
/// Untouched scopes in the cost fixture's base. Large enough that inherited
/// data dominates per-generation bookkeeping, small enough to stay a unit-speed
/// test.
const COST_FIXTURE_FILLER_SCOPES: usize = 400;

#[expect(
    clippy::expect_used,
    reason = "static fixture IDs provably satisfy the canonical ID policy"
)]
fn repo() -> RepoId {
    RepoId::new("carryforward-repo").expect("static fixture ID satisfies canonical policy")
}

#[expect(
    clippy::expect_used,
    reason = "static fixture IDs provably satisfy the canonical ID policy"
)]
fn revision() -> RevisionId {
    RevisionId::new("carryforward-rev").expect("static fixture ID satisfies canonical policy")
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
    current_source_fixture::text_scope(
        &repo(),
        &revision(),
        path,
        language.clone(),
        body,
        vec![ChunkRecord {
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
    )
}

fn base_batch(generation: ManifestGeneration) -> Result<SearchCorpusIngestBatch, Box<dyn Error>> {
    base_batch_with_filler(generation, 0)
}

/// The two named scopes the correctness tests assert on, plus `filler_scopes`
/// unrelated scopes.
///
/// The filler exists so the cost test has a base whose bulk is genuinely
/// untouched by the delta. With only the two named scopes, per-generation
/// bookkeeping (`meta.json` alone is ~4 KiB) dominates the byte count and the
/// ratio says nothing about whether unchanged data was rewritten.
fn base_batch_with_filler(
    generation: ManifestGeneration,
    filler_scopes: usize,
) -> Result<SearchCorpusIngestBatch, Box<dyn Error>> {
    let mut replace_scopes = vec![
        scope(ALPHA_PATH, "chunk-alpha", ALPHA_MARKER)?,
        scope(
            BETA_PATH,
            "chunk-beta",
            &format!("{BETA_MARKER} {BETA_RETIRED_WORD}"),
        )?,
    ];
    for index in 0..filler_scopes {
        replace_scopes.push(scope(
            &format!("src/filler/mod_{index:05}.rs"),
            &format!("chunk-filler-{index:05}"),
            &format!(
                "fn filler_{index:05}() {{ let token = quartz_{index:05}; lorem ipsum dolor sit amet consectetur adipiscing elit sed do eiusmod tempor }}"
            ),
        )?);
    }
    replace_scopes.sort_by(|left, right| {
        left.coverage
            .source
            .file
            .repo_relative_path
            .as_str()
            .cmp(right.coverage.source.file.repo_relative_path.as_str())
    });
    let mut batch = SearchCorpusIngestBatch {
        source_event: current_source_fixture::empty_event(),
        repo_id: repo(),
        revision_id: revision(),
        generation,
        base_generation: None,
        manifest_digest: format!("carryforward-manifest:{}", generation.get()),
        batch_digest: format!("carryforward-batch:{}", generation.get()),
        mode: BatchIngestMode::ReplaceGeneration,
        bundle_payload: None,
        clear_surfaces: Vec::new(),
        replace_scopes,
        tombstone_scopes: Vec::new(),
        semantic_replace_scopes: Vec::new(),
        semantic_tombstone_scopes: Vec::new(),
        seal: true,
    };
    current_source_fixture::finish_batch(&mut batch)?;
    Ok(batch)
}

/// Replaces only `src/beta.rs`; `src/alpha.rs` must survive from the base.
fn delta_batch(
    generation: ManifestGeneration,
    base: ManifestGeneration,
) -> Result<SearchCorpusIngestBatch, Box<dyn Error>> {
    let mut batch = SearchCorpusIngestBatch {
        source_event: current_source_fixture::empty_event(),
        repo_id: repo(),
        revision_id: revision(),
        generation,
        base_generation: Some(base),
        manifest_digest: format!("carryforward-manifest:{}", generation.get()),
        batch_digest: format!("carryforward-batch:{}", generation.get()),
        mode: BatchIngestMode::Delta,
        bundle_payload: None,
        clear_surfaces: Vec::new(),
        replace_scopes: vec![scope(
            BETA_PATH,
            "chunk-beta",
            &format!("{BETA_MARKER_V2} {BETA_FRESH_WORD}"),
        )?],
        tombstone_scopes: Vec::new(),
        semantic_replace_scopes: Vec::new(),
        semantic_tombstone_scopes: Vec::new(),
        seal: true,
    };
    current_source_fixture::finish_batch(&mut batch)?;
    Ok(batch)
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
        .search(&keyword_query(term), 16, &RequestBudgetV1::unbounded())?
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

#[expect(
    clippy::expect_used,
    reason = "static fixture IDs provably satisfy the canonical ID policy"
)]
fn recency_batch(generation: ManifestGeneration) -> RepoCommitRecencyIngestBatch {
    RepoCommitRecencyIngestBatch {
        repo_id: repo(),
        revision_id: revision(),
        generation,
        batch_digest: format!("carryforward-recency:{}", generation.get()),
        entries: vec![RepoCommitRecencyEntry {
            source_repo_id: RepoId::new("source-repo")
                .expect("static fixture ID satisfies canonical policy"),
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

/// The generation-local directory holding the text authority: its manifest
/// and its shard files.
const TEXT_AUTHORITY_DIR: &str = "text-authority";
const SOURCE_FILE_COVERAGE_FILE: &str = "source-file-coverage.cbor";

/// The text-authority files under `root`, sorted, with their sizes.
fn text_authority_files(root: &Path) -> Result<Vec<(std::path::PathBuf, u64)>, Box<dyn Error>> {
    let mut files = Vec::new();
    for entry in std::fs::read_dir(root.join(TEXT_AUTHORITY_DIR))? {
        let entry = entry?;
        files.push((entry.path(), entry.metadata()?.len()));
    }
    files.sort();
    Ok(files)
}

/// Total bytes of the per-generation text authority under `root`.
fn text_authority_bytes(root: &Path) -> Result<u64, Box<dyn Error>> {
    Ok(text_authority_files(root)?
        .iter()
        .fold(0_u64, |total, (_, len)| total.saturating_add(*len)))
}

/// Machine-readable cost evidence; visible with `-- --nocapture`.
#[expect(
    clippy::print_stdout,
    reason = "QI-BB-006 is a cost claim, so the measured byte counts belong in the run log the finding cites"
)]
fn emit_evidence(fields: &[(&str, String)]) {
    let rendered: Vec<String> = fields
        .iter()
        .map(|(key, value)| format!("{key}={value}"))
        .collect();
    println!("QI-BB-006-EVIDENCE {}", rendered.join(" "));
}

/// One entry's path relative to the generation directory and its size, as
/// the cost breakdown reports it.
type FreshEntry = (String, u64);

/// A generation-directory sidecar's `(inode, length, sha256)`.
type SidecarFacts = (u64, u64, String);

/// Bytes under `root` that do not share storage with `shared_inodes`, plus the
/// per-entry breakdown so a regression names what was rewritten.
fn bytes_not_shared_with(
    root: &Path,
    shared_inodes: &BTreeSet<u64>,
) -> Result<(u64, Vec<FreshEntry>), Box<dyn Error>> {
    let mut fresh = 0_u64;
    let mut entries: Vec<FreshEntry> = Vec::new();
    let mut pending = vec![root.to_path_buf()];
    while let Some(directory) = pending.pop() {
        for entry in std::fs::read_dir(&directory)? {
            let entry = entry?;
            let metadata = entry.metadata()?;
            if metadata.is_dir() {
                pending.push(entry.path());
                continue;
            }
            if metadata.is_file() && !shared_inodes.contains(&metadata.ino()) {
                fresh = fresh.saturating_add(metadata.len());
                entries.push((
                    entry
                        .path()
                        .strip_prefix(root)?
                        .to_string_lossy()
                        .into_owned(),
                    metadata.len(),
                ));
            }
        }
    }
    entries.sort_by(|left, right| right.1.cmp(&left.1).then_with(|| left.0.cmp(&right.0)));
    Ok((fresh, entries))
}

fn inodes_and_bytes(root: &Path) -> Result<(BTreeSet<u64>, u64), Box<dyn Error>> {
    let mut inodes = BTreeSet::new();
    let mut total = 0_u64;
    let mut pending = vec![root.to_path_buf()];
    while let Some(directory) = pending.pop() {
        for entry in std::fs::read_dir(&directory)? {
            let entry = entry?;
            let metadata = entry.metadata()?;
            if metadata.is_dir() {
                pending.push(entry.path());
                continue;
            }
            if metadata.is_file() {
                let _inserted = inodes.insert(metadata.ino());
                total = total.saturating_add(metadata.len());
            }
        }
    }
    Ok((inodes, total))
}

/// QI-BB-006: a one-scope delta must not rewrite the inherited index.
///
/// Text authority has a separate inode oracle. Coverage is an independent,
/// generation-bound full-snapshot artifact, so report its rewrite cost rather
/// than silently counting it as index bytes or pretending this test bounds it.
#[test]
fn delta_generation_does_not_rewrite_unchanged_index_bytes() -> TestResult {
    let dir = tempfile::tempdir()?;
    let adapter = LexicalAdapter::with_state_root(dir.path().to_path_buf());
    let g1 = ManifestGeneration::new(1);
    let g2 = ManifestGeneration::new(2);

    adapter.build_batch(&base_batch_with_filler(g1, COST_FIXTURE_FILLER_SCOPES)?)?;
    let base_dir = generation_dir(dir.path(), g1)?;
    let (base_inodes, base_bytes) = inodes_and_bytes(&base_dir)?;

    adapter.build_batch(&delta_batch(g2, g1)?)?;
    let delta_dir = generation_dir(dir.path(), g2)?;
    let (fresh_bytes, fresh_entries) = bytes_not_shared_with(&delta_dir, &base_inodes)?;

    // The base must still be intact and serving.
    assert_hits(
        &adapter,
        g1,
        ALPHA_MARKER,
        &["chunk-alpha"],
        "base after delta",
    )?;
    assert_hits(
        &adapter,
        g1,
        BETA_MARKER,
        &["chunk-beta"],
        "base after delta",
    )?;
    // The delta must be correct.
    assert_hits(&adapter, g2, ALPHA_MARKER, &["chunk-alpha"], "delta")?;
    assert_hits(&adapter, g2, BETA_MARKER_V2, &["chunk-beta"], "delta")?;

    let base_sidecar_bytes = text_authority_bytes(&base_dir)?;
    let base_coverage_bytes = std::fs::metadata(base_dir.join(SOURCE_FILE_COVERAGE_FILE))?.len();
    let fresh_coverage_bytes = fresh_entries
        .iter()
        .find(|(name, _)| name == SOURCE_FILE_COVERAGE_FILE)
        .map(|(_, len)| *len)
        .ok_or("delta coverage artifact did not get its own generation-bound storage")?;
    let breakdown: Vec<String> = fresh_entries
        .iter()
        .map(|(name, len)| format!("{name}:{len}"))
        .collect();
    emit_evidence(&[
        ("base_bytes", base_bytes.to_string()),
        ("base_text_authority_bytes", base_sidecar_bytes.to_string()),
        ("base_coverage_bytes", base_coverage_bytes.to_string()),
        ("delta_fresh_bytes", fresh_bytes.to_string()),
        (
            "delta_fresh_coverage_bytes",
            fresh_coverage_bytes.to_string(),
        ),
        ("delta_fresh_entries", breakdown.join(",")),
    ]);

    if base_bytes == 0 {
        return Err("base generation wrote no bytes; the measurement is vacuous".into());
    }
    // Scope: indexed data only. The text-authority half has its own inode
    // oracle. The full coverage snapshot's rewrite is a separate cost gap.
    let index_fresh_bytes: u64 = fresh_entries
        .iter()
        .filter(|(name, _)| {
            !name.starts_with(TEXT_AUTHORITY_DIR) && name != SOURCE_FILE_COVERAGE_FILE
        })
        .fold(0_u64, |total, (_, len)| total.saturating_add(*len));
    let index_base_bytes = base_bytes
        .checked_sub(base_sidecar_bytes)
        .and_then(|bytes| bytes.checked_sub(base_coverage_bytes))
        .ok_or("base index accounting exceeds total generation bytes")?;

    if index_base_bytes == 0 {
        return Err("base generation wrote no index bytes; the measurement is vacuous".into());
    }
    let budget = index_base_bytes.saturating_div(2);
    if index_fresh_bytes > budget {
        return Err(format!(
            "delta generation wrote {index_fresh_bytes} fresh index bytes against a \
             {index_base_bytes}-byte base index (budget {budget}): unchanged base data was \
             rewritten rather than inherited. fresh entries: {breakdown:?}"
        )
        .into());
    }
    Ok(())
}

/// Resolves the on-disk directory for one generation of the fixture corpus.
///
/// The adapter owns its layout, so the test discovers the directory instead of
/// reconstructing the hash: exactly one repo/revision root exists under the
/// state root for this fixture.
fn generation_dir(
    state_root: &Path,
    generation: ManifestGeneration,
) -> Result<std::path::PathBuf, Box<dyn Error>> {
    let mut roots: Vec<std::path::PathBuf> = Vec::new();
    for entry in std::fs::read_dir(state_root)? {
        let entry = entry?;
        if entry.file_type()?.is_dir() {
            roots.push(entry.path());
        }
    }
    let [root] = roots.as_slice() else {
        return Err(format!(
            "expected exactly one corpus root under the state root, got {roots:?}"
        )
        .into());
    };
    let path = root.join(format!("g{}", generation.get()));
    if !path.is_dir() {
        return Err(format!("generation directory {} does not exist", path.display()).into());
    }
    Ok(path)
}

/// A delta must not mutate the base generation's text-authority files.
///
/// The touched shards and the manifest are rewritten whenever a batch
/// touches indexed text. They used to be written with `File::create`, which
/// truncates in place: once a delta generation shares storage with its base
/// (materialization inherits unchanged files), that rewrite mutated the
/// base's bytes out from under readers still pinned to it. Every file is
/// published by atomic rename now, and this test is the regression: it
/// compares the base's file identities and sizes across a delta that
/// rewrites its one shard and its manifest.
#[test]
fn delta_generation_does_not_mutate_base_text_authority_sidecars() -> TestResult {
    let dir = tempfile::tempdir()?;
    let adapter = LexicalAdapter::with_state_root(dir.path().to_path_buf());
    let g1 = ManifestGeneration::new(1);
    let g2 = ManifestGeneration::new(2);

    adapter.build_batch(&base_batch(g1)?)?;
    let base_dir = generation_dir(dir.path(), g1)?;
    let base_files: Vec<std::path::PathBuf> = text_authority_files(&base_dir)?
        .into_iter()
        .map(|(path, _)| path)
        .collect();
    let before = sidecar_facts(&base_files)?;

    adapter.build_batch(&delta_batch(g2, g1)?)?;
    let after = sidecar_facts(&base_files)?;

    if before != after {
        return Err(format!(
            "delta rebuild mutated the base generation's text-authority sidecars: \
             before={before:?} after={after:?}"
        )
        .into());
    }

    // The base must still answer from those sidecars exactly as it did.
    assert_hits(
        &adapter,
        g1,
        BETA_MARKER,
        &["chunk-beta"],
        "base after delta rebuild",
    )?;
    Ok(())
}

/// `(inode, length, sha256)` for each file in `paths`.
fn sidecar_facts(paths: &[std::path::PathBuf]) -> Result<Vec<SidecarFacts>, Box<dyn Error>> {
    let mut facts = Vec::with_capacity(paths.len());
    for path in paths {
        let metadata = std::fs::metadata(path)?;
        let digest = Sha256::digest(std::fs::read(path)?);
        let mut encoded = String::with_capacity(digest.len().saturating_mul(2));
        for byte in digest {
            write!(&mut encoded, "{byte:02x}")?;
        }
        facts.push((metadata.ino(), metadata.len(), encoded));
    }
    Ok(facts)
}

fn leaf_query(leaf: LqLeaf) -> LqQuery {
    LqQuery {
        lq_version: LQ_VERSION_TAG,
        expr: LqExpr::Leaf(leaf),
        filters: Vec::new(),
        options: LqOptions::defaults(),
        directives: Vec::new(),
        source_span: LqSpan::eof(0),
    }
}

fn leaf_hit_ids(
    adapter: &LexicalAdapter,
    generation: ManifestGeneration,
    leaf: LqLeaf,
) -> Result<Vec<String>, Box<dyn Error>> {
    let searcher = adapter.open(&repo(), &revision(), generation)?;
    let mut ids: Vec<String> = searcher
        .search(&leaf_query(leaf), 64, &RequestBudgetV1::unbounded())?
        .iter()
        .map(|candidate| candidate.candidate_id.clone())
        .collect();
    ids.sort();
    Ok(ids)
}

/// DA-06: the incrementally updated text authority must answer exactly as an
/// independent full rebuild of the same final corpus.
///
/// Regex and phrase leaves are answered from the text-authority sidecars, not
/// from Tantivy postings, so this is the route that would diverge if the
/// in-place sidecar update dropped a retired document, kept a stale one, or
/// mis-assigned a doc id. The oracle (g9) is built from scratch with no base.
#[test]
fn delta_generation_text_authority_matches_independent_full_rebuild() -> TestResult {
    const FILLERS: usize = 40;
    let dir = tempfile::tempdir()?;
    let adapter = LexicalAdapter::with_state_root(dir.path().to_path_buf());
    let g1 = ManifestGeneration::new(1);
    let g2 = ManifestGeneration::new(2);
    let g9 = ManifestGeneration::new(9);

    // Base: alpha + beta + fillers.
    adapter.build_batch(&base_batch_with_filler(g1, FILLERS)?)?;

    // Delta: replace beta, tombstone filler 7, add a brand-new scope.
    let mut delta = delta_batch(g2, g1)?;
    delta.replace_scopes.push(scope(
        "src/zeta.rs",
        "chunk-zeta",
        "zeta_marker novelword appears only in the delta",
    )?);
    delta.replace_scopes.sort_by(|left, right| {
        left.coverage
            .source
            .file
            .repo_relative_path
            .as_str()
            .cmp(right.coverage.source.file.repo_relative_path.as_str())
    });
    delta
        .tombstone_scopes
        .push(quanta_index_contract::SearchCorpusTombstoneScope {
            file: quanta_index_contract::SourceFileKey {
                source_repo_id: repo(),
                repo_relative_path: RepoRelativePath::new("src/filler/mod_00007.rs"),
            },
        });
    current_source_fixture::finish_batch(&mut delta)?;
    adapter.build_batch(&delta)?;

    // Oracle: the same final content, built fresh with no base.
    let mut oracle = base_batch_with_filler(g9, FILLERS)?;
    oracle.replace_scopes.retain(|scope| {
        let path = scope.coverage.source.file.repo_relative_path.as_str();
        path != "src/filler/mod_00007.rs" && path != BETA_PATH
    });
    oracle.replace_scopes.push(scope(
        BETA_PATH,
        "chunk-beta",
        &format!("{BETA_MARKER_V2} {BETA_FRESH_WORD}"),
    )?);
    oracle.replace_scopes.push(scope(
        "src/zeta.rs",
        "chunk-zeta",
        "zeta_marker novelword appears only in the delta",
    )?);
    oracle.replace_scopes.sort_by(|left, right| {
        left.coverage
            .source
            .file
            .repo_relative_path
            .as_str()
            .cmp(right.coverage.source.file.repo_relative_path.as_str())
    });
    current_source_fixture::finish_batch(&mut oracle)?;
    adapter.build_batch(&oracle)?;

    let probes: Vec<(&str, LqLeaf)> = vec![
        (
            "regex retired",
            LqLeaf::Regex(BETA_RETIRED_WORD.to_string()),
        ),
        ("regex fresh", LqLeaf::Regex(BETA_FRESH_WORD.to_string())),
        ("regex alpha", LqLeaf::Regex(ALPHA_MARKER.to_string())),
        ("regex novel", LqLeaf::Regex("novelword".to_string())),
        (
            "regex filler prefix",
            LqLeaf::Regex("quartz_0000.".to_string()),
        ),
        (
            "regex tombstoned filler",
            LqLeaf::Regex("quartz_00007".to_string()),
        ),
        (
            "regex all fillers",
            LqLeaf::Regex("filler_[0-9]+".to_string()),
        ),
        (
            "phrase fresh",
            LqLeaf::Phrase(format!("{BETA_MARKER_V2} {BETA_FRESH_WORD}")),
        ),
        (
            "phrase retired",
            LqLeaf::Phrase(format!("{BETA_MARKER} {BETA_RETIRED_WORD}")),
        ),
        (
            "phrase novel",
            LqLeaf::Phrase("novelword appears".to_string()),
        ),
        ("keyword novel", LqLeaf::Keyword("novelword".to_string())),
    ];

    let mut divergences: Vec<String> = Vec::new();
    for (label, leaf) in probes {
        let incremental = leaf_hit_ids(&adapter, g2, leaf.clone())?;
        let rebuilt = leaf_hit_ids(&adapter, g9, leaf)?;
        if incremental != rebuilt {
            divergences.push(format!(
                "{label}: incremental={incremental:?} rebuild={rebuilt:?}"
            ));
        }
    }
    if !divergences.is_empty() {
        return Err(format!(
            "incremental text authority diverged from the independent rebuild:\n  {}",
            divergences.join("\n  ")
        )
        .into());
    }

    // Spot-check the oracle itself so agreement is not agreement on nothing.
    assert_hits(&adapter, g9, BETA_FRESH_WORD, &["chunk-beta"], "oracle")?;
    if leaf_hit_ids(&adapter, g9, LqLeaf::Regex(BETA_RETIRED_WORD.to_string()))?
        != Vec::<String>::new()
    {
        return Err("oracle must not contain the retired text".into());
    }
    if leaf_hit_ids(&adapter, g9, LqLeaf::Regex("quartz_00007".to_string()))?
        != Vec::<String>::new()
    {
        return Err("oracle must not contain the tombstoned filler".into());
    }
    if leaf_hit_ids(&adapter, g9, LqLeaf::Regex("filler_[0-9]+".to_string()))?.len() != FILLERS - 1
    {
        return Err("oracle must hold every filler but the tombstoned one".into());
    }
    Ok(())
}

/// Cost shape of the text-authority update: in-place delta vs full rebuild.
///
/// The oracle is the adapter's own work count, not a clock: a one-scope
/// delta derives exactly the one chunk the scope carries, retires exactly
/// the one it replaced and writes exactly the one shard both live in, while
/// a fresh build of the identical final corpus derives every document. Wall
/// times are printed for the ledger only; on a shared host they are a
/// shape, not an assertion.
#[test]
fn delta_text_authority_update_derives_only_the_changed_scope() -> TestResult {
    const FILLERS: usize = 1_500;
    let dir = tempfile::tempdir()?;
    let adapter = LexicalAdapter::with_state_root(dir.path().to_path_buf());
    let g1 = ManifestGeneration::new(1);
    let g2 = ManifestGeneration::new(2);
    let g9 = ManifestGeneration::new(9);

    adapter.build_batch(&base_batch_with_filler(g1, FILLERS)?)?;
    let after_base = adapter.text_authority_update_stats()?;
    let total_docs = u64::try_from(FILLERS.saturating_add(2))?;
    if after_base
        != (TextAuthorityUpdateStats {
            rebuilds: 1,
            incremental_updates: 0,
            docs_derived: total_docs,
            docs_retired: 0,
            shards_written: 1,
            shards_inherited: 0,
        })
    {
        return Err(format!(
            "a fresh generation rebuilds its text authority from every document into one shard: {after_base:?}"
        )
        .into());
    }

    let delta_started = std::time::Instant::now();
    adapter.build_batch(&delta_batch(g2, g1)?)?;
    let delta_wall = delta_started.elapsed();
    let after_delta = adapter.text_authority_update_stats()?;
    let delta_work = TextAuthorityUpdateStats {
        rebuilds: after_delta.rebuilds.saturating_sub(after_base.rebuilds),
        incremental_updates: after_delta
            .incremental_updates
            .saturating_sub(after_base.incremental_updates),
        docs_derived: after_delta
            .docs_derived
            .saturating_sub(after_base.docs_derived),
        docs_retired: after_delta
            .docs_retired
            .saturating_sub(after_base.docs_retired),
        shards_written: after_delta
            .shards_written
            .saturating_sub(after_base.shards_written),
        shards_inherited: after_delta
            .shards_inherited
            .saturating_sub(after_base.shards_inherited),
    };
    // The corpus fits one shard, so the one touched shard is the one
    // written; the multi-shard shape is the shard test's oracle.
    if delta_work
        != (TextAuthorityUpdateStats {
            rebuilds: 0,
            incremental_updates: 1,
            docs_derived: 1,
            docs_retired: 1,
            shards_written: 1,
            shards_inherited: 0,
        })
    {
        return Err(format!(
            "a one-scope delta derives one chunk and retires one, never the corpus: {delta_work:?}"
        )
        .into());
    }

    let mut full = base_batch_with_filler(g9, FILLERS)?;
    full.replace_scopes
        .retain(|scope| scope.coverage.source.file.repo_relative_path.as_str() != BETA_PATH);
    full.replace_scopes.push(scope(
        BETA_PATH,
        "chunk-beta",
        &format!("{BETA_MARKER_V2} {BETA_FRESH_WORD}"),
    )?);
    full.replace_scopes.sort_by(|left, right| {
        left.coverage
            .source
            .file
            .repo_relative_path
            .as_str()
            .cmp(right.coverage.source.file.repo_relative_path.as_str())
    });
    current_source_fixture::finish_batch(&mut full)?;
    let full_started = std::time::Instant::now();
    adapter.build_batch(&full)?;
    let full_wall = full_started.elapsed();
    let after_full = adapter.text_authority_update_stats()?;
    let full_work = after_full
        .docs_derived
        .saturating_sub(after_delta.docs_derived);
    if after_full.rebuilds.saturating_sub(after_delta.rebuilds) != 1 || full_work != total_docs {
        return Err(format!(
            "the independent build derives every one of the {total_docs} documents: {after_full:?}"
        )
        .into());
    }

    let base_dir = generation_dir(dir.path(), g1)?;
    let sidecar_bytes = text_authority_bytes(&base_dir)?;
    emit_evidence(&[
        ("scopes", FILLERS.saturating_add(2).to_string()),
        ("text_authority_bytes", sidecar_bytes.to_string()),
        ("delta_docs_derived", delta_work.docs_derived.to_string()),
        ("full_docs_derived", full_work.to_string()),
        ("delta_one_scope_ms", delta_wall.as_millis().to_string()),
        ("full_rebuild_ms", full_wall.as_millis().to_string()),
    ]);
    Ok(())
}
