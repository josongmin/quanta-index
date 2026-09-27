//! QI-BB-006 보완 #4 — a seal reads bytes proportional to what the
//! generation changed, not to its size.
//!
//! The seal commits every file a query opens with its length and SHA-256.
//! A fresh generation is measured whole; a delta inherits the base's
//! commitment for every file that *is* the base's inode (the segment
//! files, text-authority shards and overlays the delta hard-linked) and
//! measures only what it wrote. These tests pin that with an inode oracle:
//! the files of the delta are partitioned on disk by whether their inode
//! is the base's, and the seal's own measurement — bytes it read through
//! its hasher, bytes it inherited — must equal the sizes of exactly those
//! partitions. Nothing here reads a clock.

#![forbid(unsafe_code)]

use std::collections::{BTreeMap, BTreeSet};
use std::error::Error;
use std::os::unix::fs::MetadataExt as _;
use std::path::{Path, PathBuf};

use quanta_index_contract::lex::LanguageCode;
use quanta_index_contract::{
    BatchIngestMode, ChunkId, ChunkRecord, LQ_VERSION_TAG, LqExpr, LqLeaf, LqOptions, LqQuery,
    LqSpan, ManifestGeneration, RepoId, RepoMetaEntry, RepoMetaIngestBatch, RepoRelativePath,
    RevisionId, SearchCorpusIngestBatch, SearchCorpusReplaceScope, SourceFileCoverage,
    SourceFileKey, SourceFileRevision, SourcePublicationEvent, SymbolCoverage,
    source_event_payload_sha256, source_file_unit_set_sha256,
};
use quanta_index_core::{
    GenerationStorageKeyV1, LexicalIndexOpenPort, RepoMetaIngestPort, RequestBudgetV1,
    SearchCorpusBatchBuildPort,
};
use quanta_index_lexical::{LexicalAdapter, LexicalSealCommitmentStats};
use sha2::{Digest as _, Sha256};

type TestResult = Result<(), Box<dyn Error>>;

/// Enough documents for several text-authority shards, so the untouched
/// shards and the base segment are the bulk of what a delta inherits.
const DOCS: usize = 3 * 2048 + 50;
const TEXT_AUTHORITY_DIR: &str = "text-authority";
const TANTIVY_META: &str = "meta.json";
const SOURCE_FILE_COVERAGE: &str = "source-file-coverage.cbor";
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
    let chunks = vec![ChunkRecord {
        chunk_id: ChunkId::new(scope_chunk(index)),
        repo_relative_path: RepoRelativePath::new(&path),
        language: language.clone(),
        start_byte: 0,
        end_byte: u32::try_from(body.len())?,
        start_line: 1,
        end_line: 1,
        text: body.to_string().into_boxed_str(),
        structural: None,
        parent_chunk_id: None,
        source_repo_id: None,
    }];
    Ok(SearchCorpusReplaceScope {
        coverage: SourceFileCoverage {
            source: SourceFileRevision {
                file: SourceFileKey {
                    source_repo_id: repo(),
                    repo_relative_path: RepoRelativePath::new(&path),
                },
                revision_id: revision(),
                source_sha256: Sha256::digest(body.as_bytes()).into(),
            },
            language,
            producer_policy_sha256: Sha256::digest(b"seal-cost-fixture-v1").into(),
            unit_set_sha256: source_file_unit_set_sha256(&chunks, &[])?,
            text_admitted: true,
            symbols: SymbolCoverage::Complete { symbol_count: 0 },
        },
        chunks,
        symbols: Vec::new(),
    })
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
        source_event: SourcePublicationEvent {
            stream_id: "seal-cost-test".into(),
            event_id: format!("event-{}", generation.get()),
            expected_base_event_id: base.map(|previous| format!("event-{}", previous.get())),
            payload_sha256: [0; 32],
        },
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
    batch.source_event.payload_sha256 = source_event_payload_sha256(&batch)?;
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
/// The Tantivy commit, every segment component and ranked-key file, every
/// file under `text-authority/` and every overlay present. Tantivy's managed list and
/// lock files, the seal's own manifest and identity and the delta marker
/// are not query content and are not committed.
fn committed_files(generation_dir: &Path) -> Result<Vec<CommittedFile>, Box<dyn Error>> {
    let mut files = Vec::new();
    for entry in std::fs::read_dir(generation_dir)? {
        let entry = entry?;
        let metadata = entry.metadata()?;
        let name = entry.file_name().to_string_lossy().into_owned();
        if metadata.is_dir() {
            if name != TEXT_AUTHORITY_DIR {
                return Err(format!("unexpected directory {name}").into());
            }
            for shard in std::fs::read_dir(entry.path())? {
                let shard = shard?;
                let shard_metadata = shard.metadata()?;
                files.push(CommittedFile {
                    name: format!(
                        "{TEXT_AUTHORITY_DIR}/{}",
                        shard.file_name().to_string_lossy()
                    ),
                    bytes: shard_metadata.len(),
                    inode: shard_metadata.ino(),
                });
            }
            continue;
        }
        let is_segment_file = name.split_once('.').is_some_and(|(stem, _)| {
            stem.len() == 32 && stem.chars().all(|ch| ch.is_ascii_hexdigit())
        });
        let is_ranked_keys = name.starts_with("ranked-keys-") && name.ends_with(".bin");
        if name == TANTIVY_META
            || name == SOURCE_FILE_COVERAGE
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
    }
}

fn hit_ids(
    adapter: &LexicalAdapter,
    generation: ManifestGeneration,
    pattern: &str,
) -> Result<Vec<String>, Box<dyn Error>> {
    let searcher = adapter.open(&repo(), &revision(), generation)?;
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

/// The bytes oracle: the base seal reads every committed byte once, and a
/// one-scope delta's seal reads exactly the files that are not the base's
/// inode and inherits exactly the ones that are.
#[test]
fn a_delta_seal_reads_only_what_the_delta_wrote() -> TestResult {
    let temp = tempfile::tempdir()?;
    let root = temp.path().to_path_buf();
    let adapter = LexicalAdapter::with_state_root(root.clone());
    let g1 = ManifestGeneration::new(1);
    let g2 = ManifestGeneration::new(2);

    // The base: a full build, measured whole.
    publish_repo_meta(&adapter, g1)?;
    let before_base = adapter.seal_commitment_stats()?;
    adapter.build_batch(&base_batch(g1)?)?;
    let base_seal = delta_stats(before_base, adapter.seal_commitment_stats()?);
    let base_files = committed_files(&generation_dir(&root, g1))?;
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
    adapter.build_batch(&batch(
        g2,
        Some(g1),
        vec![scope(replaced, "fn replaced() { replacedsentinel }")?],
    )?)?;
    let delta_seal = delta_stats(before_delta, adapter.seal_commitment_stats()?);
    let delta_files = committed_files(&generation_dir(&root, g2))?;
    let (linked, written): (Vec<&CommittedFile>, Vec<&CommittedFile>) = delta_files
        .iter()
        .partition(|file| base_inodes.contains(&file.inode));
    let linked_bytes = total_bytes(linked.iter().copied());
    let written_bytes = total_bytes(written.iter().copied());
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
    if delta_seal.files_hashed != u64::try_from(written.len())?
        || delta_seal.bytes_hashed != written_bytes
    {
        return Err(format!(
            "the delta seal hashed {} files / {} bytes; disk says {} files / {written_bytes} bytes are not the base's inode ({written_names:?})",
            delta_seal.files_hashed,
            delta_seal.bytes_hashed,
            written.len()
        )
        .into());
    }
    if delta_seal.files_inherited != u64::try_from(linked.len())?
        || delta_seal.bytes_inherited != linked_bytes
    {
        return Err(format!(
            "the delta seal inherited {} files / {} bytes; disk says {} files / {linked_bytes} bytes are the base's inode",
            delta_seal.files_inherited,
            delta_seal.bytes_inherited,
            linked.len()
        )
        .into());
    }
    // Every inherited file is the base's file under the same name and
    // length, so the base seal's proof is the proof carried.
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
        .any(|file| file.name.starts_with(TEXT_AUTHORITY_DIR))
    {
        return Err(
            "no text-authority shard was inherited; untouched shards were rewritten".into(),
        );
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
    // The delta's read is a fraction of the base: the budget the ticket
    // names, on this fixture, is well under half.
    let budget = base_bytes.saturating_div(2);
    if delta_seal.bytes_hashed > budget {
        return Err(format!(
            "the delta seal read {} bytes against a {base_bytes}-byte base (budget {budget})",
            delta_seal.bytes_hashed
        )
        .into());
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
    Ok(())
}
