//! QI-BB-006 보완 #4 — commitment hashing reads bytes proportional to what the
//! generation changed, not to its size.
//!
//! The seal commits every file a query opens with its length and SHA-256.
//! A fresh generation is measured whole; a delta inherits the base's
//! commitment for every file that *is* the base's inode (the segment
//! files, text-authority shards, file-authority objects and overlays the delta hard-linked) and
//! measures only what it wrote. These tests pin that with an inode oracle:
//! the generation-local files of the delta are partitioned on disk by whether
//! their inode is the base's, and the seal's own measurement — bytes it read
//! through its hasher, bytes it inherited — must equal those partitions.
//! Shared File and Native pool bodies have separate physical reuse oracles.
//! Nothing here reads a clock.
//! File admission verifies changed raw source input at seal; its logical
//! counters are separate from generation-local hashing and pool object I/O.

#![forbid(unsafe_code)]

#[path = "support/publication_fixture.rs"]
mod publication_fixture;
use publication_fixture::FixtureBatchBuildPort;

use std::collections::{BTreeMap, BTreeSet};
use std::error::Error;
use std::io::Read as _;
use std::os::unix::fs::MetadataExt as _;
use std::path::{Path, PathBuf};

use sha2::{Digest as _, Sha256};

use quanta_index_contract::lex::LanguageCode;
use quanta_index_contract::{
    BatchIngestMode, ChunkId, ChunkRecord, LQ_VERSION_TAG, LqExpr, LqLeaf, LqOptions, LqQuery,
    LqSpan, ManifestGeneration, RepoId, RepoMetaEntry, RepoMetaIngestBatch, RepoRelativePath,
    RevisionId, SearchCorpusIngestBatch, SearchCorpusReplaceScope,
};
use quanta_index_core::{
    GenerationStorageKeyV1, LexicalIndexOpenPort, MetricSourcePort, MetricValueV1,
    RepoMetaIngestPort, RequestBudgetV1,
};
use quanta_index_lexical::{LexicalAdapter, LexicalSealCommitmentStats};

#[path = "support/current_source_fixture.rs"]
mod current_source_fixture;

type TestResult = Result<(), Box<dyn Error>>;

/// Enough documents for several bounded Native segments and Tantivy segments.
const DOCS: usize = 3 * 2048 + 50;
const TEXT_AUTHORITY_DIR: &str = "text-authority";
const FILE_AUTHORITY_DIR: &str = "file-authority";
const SOURCE_FILE_COVERAGE: &str = "source-file-coverage.cbor";
const TANTIVY_META: &str = "meta.json";
const LIVE_BM25_STATS: &str = "search-corpus-live-bm25.cbor";
const OVERLAY_FILES: [&str; 7] = [
    "repo-metadata.cbor",
    "repo-commit-recency.cbor",
    "repo-meta.cbor",
    "repo-topic.cbor",
    "repo-description.cbor",
    "file-ownership.cbor",
    "file-contributor.cbor",
];

#[expect(
    clippy::expect_used,
    reason = "static fixture IDs provably satisfy the canonical ID policy"
)]
fn repo() -> RepoId {
    RepoId::new("cost-repo").expect("static fixture ID satisfies canonical policy")
}

#[expect(
    clippy::expect_used,
    reason = "static fixture IDs provably satisfy the canonical ID policy"
)]
fn revision() -> RevisionId {
    RevisionId::new("cost-rev").expect("static fixture ID satisfies canonical policy")
}

fn scope_path(index: usize) -> String {
    format!("src/c/{index:05}.rs")
}

fn scope_chunk(index: usize) -> String {
    format!("chunk-{index:05}")
}

fn scope_body(index: usize) -> String {
    format!("fn scope_{index:05}() {{ let token = quartz_{index:05}; lorem ipsum dolor sit amet }}")
}

fn scope(index: usize, body: &str) -> Result<SearchCorpusReplaceScope, Box<dyn Error>> {
    let language = LanguageCode::new("rust")
        .map_err(|err| -> Box<dyn Error> { format!("language code: {err}").into() })?;
    let path = scope_path(index);
    current_source_fixture::text_scope(
        &repo(),
        &revision(),
        &path,
        language.clone(),
        body,
        vec![ChunkRecord {
            chunk_id: ChunkId::new(scope_chunk(index)),
            repo_relative_path: RepoRelativePath::new(&path),
            language,
            start_byte: 0,
            end_byte: u32::try_from(body.len())?,
            start_line: 1,
            end_line: 1,
            text: body.to_string().into_boxed_str(),
            structural: None,
            parent_chunk_id: None,
            source_repo_id: None,
        }],
    )
}

fn batch(
    generation: ManifestGeneration,
    base: Option<ManifestGeneration>,
    mut replace_scopes: Vec<SearchCorpusReplaceScope>,
) -> Result<SearchCorpusIngestBatch, Box<dyn Error>> {
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
        base_generation: base,
        manifest_digest: format!("cost-manifest:{}", generation.get()),
        batch_digest: "0".repeat(64),
        mode: if base.is_some() {
            BatchIngestMode::Delta
        } else {
            BatchIngestMode::ReplaceGeneration
        },
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

fn base_batch(generation: ManifestGeneration) -> Result<SearchCorpusIngestBatch, Box<dyn Error>> {
    let mut scopes = Vec::with_capacity(DOCS);
    for index in 0..DOCS {
        scopes.push(scope(index, &scope_body(index))?);
    }
    batch(generation, None, scopes)
}

fn generation_dir(root: &Path, generation: ManifestGeneration) -> PathBuf {
    GenerationStorageKeyV1::for_repo_revision(&repo(), &revision()).generation_dir(root, generation)
}

#[derive(Debug, Eq, PartialEq)]
struct PooledBodyFacts {
    path: PathBuf,
    inode: u64,
    bytes: u64,
    sha256: [u8; 32],
}

/// Locate one physical body by a fixed witness, without relying on either
/// authority's map or decoding its private root format.
fn pooled_body(root: &Path, kind: u8, witness: &[u8]) -> Result<PooledBodyFacts, Box<dyn Error>> {
    let mut matches = Vec::new();
    for shard in std::fs::read_dir(root.join("lexical-object-pool/objects"))? {
        let shard = shard?;
        if !shard.file_type()?.is_dir() {
            continue;
        }
        for entry in std::fs::read_dir(shard.path())? {
            let entry = entry?;
            if !entry.file_type()?.is_file() {
                continue;
            }
            let path = entry.path();
            let mut header = [0_u8; 11];
            std::fs::File::open(&path)?.read_exact(&mut header)?;
            if header.get(..8) != Some(b"QLSTOR01".as_slice()) || header.get(10) != Some(&kind) {
                continue;
            }
            let raw = std::fs::read(&path)?;
            if !raw.windows(witness.len()).any(|bytes| bytes == witness) {
                continue;
            }
            let metadata = entry.metadata()?;
            matches.push(PooledBodyFacts {
                path,
                inode: metadata.ino(),
                bytes: metadata.len(),
                sha256: Sha256::digest(raw).into(),
            });
        }
    }
    let [found] = matches.as_slice() else {
        return Err(format!(
            "expected one pooled body kind {kind} for fixed witness, got {matches:?}"
        )
        .into());
    };
    Ok(PooledBodyFacts {
        path: found.path.clone(),
        inode: found.inode,
        bytes: found.bytes,
        sha256: found.sha256,
    })
}

/// One overlay so the delta has an overlay to inherit.
#[expect(
    clippy::expect_used,
    reason = "static fixture IDs provably satisfy the canonical ID policy"
)]
fn publish_repo_meta(adapter: &LexicalAdapter, generation: ManifestGeneration) -> TestResult {
    let _receipt = adapter.publish_batch(&RepoMetaIngestBatch {
        repo_id: repo(),
        revision_id: revision(),
        generation,
        batch_digest: format!("cost-meta:{}", generation.get()),
        entries: vec![RepoMetaEntry {
            source_repo_id: RepoId::new("cost-source")
                .expect("static fixture ID satisfies canonical policy"),
            key: "lifecycle".to_string(),
            value: "cost".to_string(),
        }],
    })?;
    Ok(())
}

/// One file the seal commits to: its name relative to the generation
/// directory, size and inode.
#[derive(Clone, Debug)]
struct CommittedFile {
    name: String,
    bytes: u64,
    inode: u64,
}

/// Every file the seal commits to, as an independent walk of the directory
/// sees it.
///
/// The Tantivy commit, every segment component, ranked-key file and committed
/// live BM25 statistics, the Native and File roots under their authority trees,
/// and every overlay present. Tantivy's managed list and
/// lock files, the seal's own manifest and identity and the delta marker
/// are not query content and are not committed.
fn committed_files(generation_dir: &Path) -> Result<Vec<CommittedFile>, Box<dyn Error>> {
    let mut files = Vec::new();
    for entry in std::fs::read_dir(generation_dir)? {
        let entry = entry?;
        let metadata = entry.path().symlink_metadata()?;
        let name = entry.file_name().to_string_lossy().into_owned();
        if metadata.file_type().is_symlink() {
            return Err(format!("committed tree contains a symlink: {name}").into());
        }
        if metadata.is_dir() {
            if name != TEXT_AUTHORITY_DIR && name != FILE_AUTHORITY_DIR {
                return Err(format!("unexpected directory {name}").into());
            }
            for shard in std::fs::read_dir(entry.path())? {
                let shard = shard?;
                let shard_metadata = shard.path().symlink_metadata()?;
                let shard_name = shard.file_name().to_string_lossy().into_owned();
                if !shard_metadata.is_file() {
                    return Err(format!("unexpected sidecar entry: {name}/{shard_name}").into());
                }
                if name == FILE_AUTHORITY_DIR && shard_name != "root.cbor" {
                    return Err(
                        format!("unexpected sealed file-authority entry: {shard_name}").into(),
                    );
                }
                files.push(CommittedFile {
                    name: format!("{name}/{shard_name}"),
                    bytes: shard_metadata.len(),
                    inode: shard_metadata.ino(),
                });
            }
            continue;
        }
        let is_segment_file = name.split_once('.').is_some_and(|(stem, _)| {
            stem.len() == 32 && stem.chars().all(|ch| ch.is_ascii_hexdigit())
        });
        let extension = Path::new(&name).extension();
        let is_ranked_keys =
            name.starts_with("ranked-keys-") && extension == Some(std::ffi::OsStr::new("bin"));
        if name == TANTIVY_META
            || name == LIVE_BM25_STATS
            || name == SOURCE_FILE_COVERAGE
            || (name.starts_with("source-file-coverage-page-")
                && extension == Some(std::ffi::OsStr::new("cbor")))
            || is_segment_file
            || is_ranked_keys
            || OVERLAY_FILES.contains(&name.as_str())
        {
            files.push(CommittedFile {
                name,
                bytes: metadata.len(),
                inode: metadata.ino(),
            });
        }
    }
    files.sort_by(|left, right| left.name.cmp(&right.name));
    Ok(files)
}

fn total_bytes<'a>(files: impl Iterator<Item = &'a CommittedFile>) -> u64 {
    files.fold(0_u64, |total, file| total.saturating_add(file.bytes))
}

fn delta_stats(
    before: LexicalSealCommitmentStats,
    after: LexicalSealCommitmentStats,
) -> LexicalSealCommitmentStats {
    LexicalSealCommitmentStats {
        seals: after.seals.saturating_sub(before.seals),
        files_hashed: after.files_hashed.saturating_sub(before.files_hashed),
        bytes_hashed: after.bytes_hashed.saturating_sub(before.bytes_hashed),
        files_inherited: after.files_inherited.saturating_sub(before.files_inherited),
        bytes_inherited: after.bytes_inherited.saturating_sub(before.bytes_inherited),
        file_admission_sources_verified: after
            .file_admission_sources_verified
            .saturating_sub(before.file_admission_sources_verified),
        file_admission_source_bytes_verified: after
            .file_admission_source_bytes_verified
            .saturating_sub(before.file_admission_source_bytes_verified),
    }
}

#[test]
#[expect(
    clippy::panic_in_result_fn,
    reason = "fixed on-disk source admission oracle in a fallible fixture"
)]
fn file_admission_counts_verified_changed_sources_separately() -> TestResult {
    let temp = tempfile::tempdir()?;
    let root = temp.path();
    let adapter = LexicalAdapter::with_state_root(root.to_path_buf());
    let g1 = ManifestGeneration::new(1);
    let g2 = ManifestGeneration::new(2);
    let original = "fn original() { firstsentinel }";
    let inherited = "fn unchanged() { secondsentinel }";
    let replacement = "fn replacement() { thirdsentinel }";
    let _stages = adapter.build_fixture_batch(&batch(
        g1,
        None,
        vec![scope(0, original)?, scope(1, inherited)?],
    )?)?;
    let base_stats = adapter.seal_commitment_stats()?;
    assert_eq!(base_stats.file_admission_sources_verified, 2);
    assert_eq!(
        base_stats.file_admission_source_bytes_verified,
        u64::try_from(original.len() + inherited.len())?
    );
    let base_original = pooled_body(root, 1, original.as_bytes())?;
    let base_inherited = pooled_body(root, 1, inherited.as_bytes())?;
    let _stages =
        adapter.build_fixture_batch(&batch(g2, Some(g1), vec![scope(0, replacement)?])?)?;
    let after = adapter.seal_commitment_stats()?;
    let delta = delta_stats(base_stats, after);
    assert_eq!(pooled_body(root, 1, original.as_bytes())?, base_original);
    assert_eq!(pooled_body(root, 1, inherited.as_bytes())?, base_inherited);
    let fresh = pooled_body(root, 1, replacement.as_bytes())?;
    assert_ne!(fresh.path, base_original.path);
    assert_ne!(fresh.path, base_inherited.path);
    assert_eq!(delta.file_admission_sources_verified, 1);
    assert_eq!(
        delta.file_admission_source_bytes_verified,
        u64::try_from(replacement.len())?
    );
    assert!(delta.files_inherited > 0);
    assert!(delta.bytes_inherited >= u64::try_from(inherited.len())?);
    let metrics = adapter.scrape()?;
    for (name, count) in [
        (
            "lexical_seal_file_admission_sources_verified_total",
            after.file_admission_sources_verified,
        ),
        (
            "lexical_seal_file_admission_source_bytes_verified_total",
            after.file_admission_source_bytes_verified,
        ),
    ] {
        assert!(metrics.iter().any(|metric| {
            metric.name == name && metric.value == MetricValueV1::Counter(count)
        }));
    }
    Ok(())
}

fn hit_ids(
    adapter: &LexicalAdapter,
    generation: ManifestGeneration,
    pattern: &str,
) -> Result<Vec<String>, Box<dyn Error>> {
    let searcher = adapter.open(
        &repo(),
        &revision(),
        generation,
        &quanta_index_core::RequestBudgetV1::unbounded(),
    )?;
    let query = LqQuery {
        lq_version: LQ_VERSION_TAG,
        expr: LqExpr::Leaf(LqLeaf::Regex(pattern.to_string())),
        filters: Vec::new(),
        options: LqOptions::defaults(),
        directives: Vec::new(),
        source_span: LqSpan::eof(0),
    };
    let mut ids: Vec<String> = searcher
        .search(&query, 64, &RequestBudgetV1::unbounded())?
        .iter()
        .map(|candidate| candidate.candidate_id.clone())
        .collect();
    ids.sort();
    Ok(ids)
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
    println!("QI-BB-006-SEAL-EVIDENCE {}", rendered.join(" "));
}

/// The bytes oracle: the base seal reads every committed byte once.
///
/// A delta rehashes even inherited coverage pages to reject in-place edits after base
/// validation, while other unchanged inodes retain their base commitments.
#[test]
fn a_delta_seal_rehashes_coverage_but_inherits_other_unmodified_files() -> TestResult {
    let temp = tempfile::tempdir()?;
    let root = temp.path().to_path_buf();
    let adapter = LexicalAdapter::with_state_root(root.clone());
    let g1 = ManifestGeneration::new(1);
    let g2 = ManifestGeneration::new(2);

    // The base: a full build, measured whole.
    publish_repo_meta(&adapter, g1)?;
    let before_base = adapter.seal_commitment_stats()?;
    let _stages = adapter.build_fixture_batch(&base_batch(g1)?)?;
    let base_seal = delta_stats(before_base, adapter.seal_commitment_stats()?);
    let base_files = committed_files(&generation_dir(&root, g1))?;
    let base_native_body = pooled_body(&root, 3, scope_body(0).as_bytes())?;
    if base_files
        .iter()
        .filter(|file| file.name == LIVE_BM25_STATS)
        .count()
        != 1
    {
        return Err("base seal omitted mandatory live BM25 statistics".into());
    }
    let base_bytes = total_bytes(base_files.iter());
    if base_seal.seals != 1
        || base_seal.files_inherited != 0
        || base_seal.files_hashed != u64::try_from(base_files.len())?
        || base_seal.bytes_hashed != base_bytes
    {
        return Err(format!(
            "the base seal must hash every committed file once: stats {base_seal:?}, disk {} files / {base_bytes} bytes",
            base_files.len()
        )
        .into());
    }
    let base_inodes: BTreeSet<u64> = base_files.iter().map(|file| file.inode).collect();
    let base_by_name: BTreeMap<&str, &CommittedFile> = base_files
        .iter()
        .map(|file| (file.name.as_str(), file))
        .collect();

    // The delta: one scope replaced, the overlay inherited untouched.
    let replaced = 100;
    let before_delta = adapter.seal_commitment_stats()?;
    let _stages = adapter.build_fixture_batch(&batch(
        g2,
        Some(g1),
        vec![scope(replaced, "fn replaced() { replacedsentinel }")?],
    )?)?;
    let delta_seal = delta_stats(before_delta, adapter.seal_commitment_stats()?);
    let delta_files = committed_files(&generation_dir(&root, g2))?;
    if pooled_body(&root, 3, scope_body(0).as_bytes())? != base_native_body {
        return Err("unchanged NativeBody pool object changed path, inode, length or SHA".into());
    }
    let (linked, written): (Vec<&CommittedFile>, Vec<&CommittedFile>) = delta_files
        .iter()
        .partition(|file| base_inodes.contains(&file.inode));
    if written
        .iter()
        .filter(|file| file.name == LIVE_BM25_STATS)
        .count()
        != 1
    {
        return Err("delta seal did not commit newly written live BM25 statistics".into());
    }
    let linked_bytes = total_bytes(linked.iter().copied());
    let written_bytes = total_bytes(written.iter().copied());
    let linked_coverage_pages: Vec<_> = linked
        .iter()
        .copied()
        .filter(|file| file.name.starts_with("source-file-coverage-page-"))
        .collect();
    let linked_coverage_bytes = total_bytes(linked_coverage_pages.iter().copied());
    let written_names: Vec<&str> = written.iter().map(|file| file.name.as_str()).collect();
    emit_evidence(&[
        ("docs", DOCS.to_string()),
        ("base_files", base_files.len().to_string()),
        ("base_bytes", base_bytes.to_string()),
        (
            "base_ranked_key_bytes",
            total_bytes(
                base_files
                    .iter()
                    .filter(|file| file.name.starts_with("ranked-keys-")),
            )
            .to_string(),
        ),
        ("base_seal_bytes_hashed", base_seal.bytes_hashed.to_string()),
        ("delta_files", delta_files.len().to_string()),
        ("delta_written_bytes", written_bytes.to_string()),
        ("delta_linked_bytes", linked_bytes.to_string()),
        (
            "delta_seal_bytes_hashed",
            delta_seal.bytes_hashed.to_string(),
        ),
        (
            "delta_seal_bytes_inherited",
            delta_seal.bytes_inherited.to_string(),
        ),
        ("delta_written", written_names.join(",")),
    ]);

    if delta_seal.seals != 1 {
        return Err(format!("expected one delta seal, stats {delta_seal:?}").into());
    }
    // The disk inventory is the byte bound: newly written files and every
    // inherited coverage page require hashing; other inherited files do not.
    if delta_seal.files_hashed != u64::try_from(written.len() + linked_coverage_pages.len())?
        || delta_seal.bytes_hashed != written_bytes.saturating_add(linked_coverage_bytes)
    {
        return Err(format!(
            "the delta seal hashed {} files / {} bytes; disk says {} new files / {written_bytes} bytes plus {} linked coverage pages / {linked_coverage_bytes} bytes ({written_names:?})",
            delta_seal.files_hashed,
            delta_seal.bytes_hashed,
            written.len(),
            linked_coverage_pages.len()
        )
        .into());
    }
    if delta_seal.files_inherited != u64::try_from(linked.len() - linked_coverage_pages.len())?
        || delta_seal.bytes_inherited != linked_bytes.saturating_sub(linked_coverage_bytes)
    {
        return Err(format!(
            "the delta seal inherited {} files / {} bytes; disk says {} non-coverage files / {} bytes share the base inode",
            delta_seal.files_inherited,
            delta_seal.bytes_inherited,
            linked.len() - linked_coverage_pages.len(),
            linked_bytes.saturating_sub(linked_coverage_bytes)
        )
        .into());
    }
    // Every linked file is the base's file under the same name and length;
    // only coverage pages are also rehashed against the prepared root.
    for file in &linked {
        match base_by_name.get(file.name.as_str()) {
            Some(base) if base.inode == file.inode && base.bytes == file.bytes => {}
            other => {
                return Err(format!(
                    "inherited {} is not the base's file of that name: {other:?}",
                    file.name
                )
                .into());
            }
        }
    }
    if !linked.iter().any(|file| file.name == "repo-meta.cbor") {
        return Err("the untouched overlay must be inherited, not re-read".into());
    }
    if !linked
        .iter()
        .any(|file| !file.name.starts_with(TEXT_AUTHORITY_DIR) && file.name != "repo-meta.cbor")
    {
        return Err("no segment file was inherited; the base index was rewritten".into());
    }
    if !linked
        .iter()
        .any(|file| file.name.starts_with("ranked-keys-"))
    {
        return Err("an unchanged segment did not inherit its ranked-key table".into());
    }
    // Both generations serve from their own commitments.
    if hit_ids(&adapter, g1, &format!("quartz_{replaced:05}"))? != vec![scope_chunk(replaced)] {
        return Err("the base must still serve the replaced scope's old text".into());
    }
    if hit_ids(&adapter, g2, &format!("quartz_{replaced:05}"))? != Vec::<String>::new() {
        return Err("the delta must not serve the replaced scope's old text".into());
    }
    if hit_ids(&adapter, g2, "replacedsentinel")? != vec![scope_chunk(replaced)] {
        return Err("the delta must serve the replacement".into());
    }
    if hit_ids(&adapter, g2, "quartz_00000")? != vec![scope_chunk(0)] {
        return Err("the delta must still serve an unchanged pooled Native body".into());
    }
    Ok(())
}
