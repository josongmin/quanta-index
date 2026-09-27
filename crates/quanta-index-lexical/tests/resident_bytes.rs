//! QI-BB-001 보완 #3 — the resident-bytes estimate a handle reports to the
//! snapshot registry is the decoded footprint, not the on-disk CBOR.
//!
//! The text authority is decoded at open into the NFC text, its folded
//! copy, expanded trigram postings and positions postings; the CBOR on
//! disk is smaller than that. The estimate must therefore exceed the
//! authority's on-disk bytes, and the mapped index files must be counted
//! by inode so a delta that hard-links its base's segments does not
//! report the shared bytes as if it owned a second copy. The oracles are
//! independent: the sidecar's file sizes and an inode-set walk.

#![forbid(unsafe_code)]

use std::collections::BTreeSet;
use std::error::Error;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};

use quanta_index_contract::lex::LanguageCode;
use quanta_index_contract::{
    BatchIngestMode, ChunkId, ChunkRecord, ManifestGeneration, RepoId, RepoRelativePath,
    RevisionId, SearchCorpusIngestBatch, SearchCorpusReplaceScope, SourceFileCoverage,
    SourceFileKey, SourceFileRevision, SourcePublicationEvent, SymbolCoverage,
    source_event_payload_sha256, source_file_unit_set_sha256,
};
use quanta_index_core::{GenerationStorageKeyV1, LexicalIndexOpenPort, SearchCorpusBatchBuildPort};
use quanta_index_lexical::LexicalAdapter;
use sha2::{Digest as _, Sha256};

type TestResult = Result<(), Box<dyn Error>>;

const TEXT_AUTHORITY_DIR: &str = "text-authority";

#[expect(
    clippy::expect_used,
    reason = "static fixture IDs provably satisfy the canonical ID policy"
)]
fn repo() -> RepoId {
    RepoId::new("resident-repo").expect("static fixture ID satisfies canonical policy")
}

#[expect(
    clippy::expect_used,
    reason = "static fixture IDs provably satisfy the canonical ID policy"
)]
fn revision() -> RevisionId {
    RevisionId::new("resident-rev").expect("static fixture ID satisfies canonical policy")
}

fn scope(path: &str, body: &str) -> Result<SearchCorpusReplaceScope, Box<dyn Error>> {
    let language = LanguageCode::new("rust")
        .map_err(|err| -> Box<dyn Error> { format!("language code: {err}").into() })?;
    let chunks = vec![ChunkRecord {
        chunk_id: ChunkId::new(format!("chunk:{path}")),
        repo_relative_path: RepoRelativePath::new(path),
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
                    repo_relative_path: RepoRelativePath::new(path),
                },
                revision_id: revision(),
                source_sha256: Sha256::digest(body.as_bytes()).into(),
            },
            language,
            producer_policy_sha256: Sha256::digest(b"resident-test-fixture-v1").into(),
            unit_set_sha256: source_file_unit_set_sha256(&chunks, &[])?,
            text_admitted: true,
            symbols: SymbolCoverage::Complete { symbol_count: 0 },
        },
        chunks,
        symbols: Vec::new(),
    })
}

fn batch(
    generation: u64,
    base: Option<u64>,
    scopes: Vec<SearchCorpusReplaceScope>,
) -> Result<SearchCorpusIngestBatch, Box<dyn Error>> {
    let mut batch = SearchCorpusIngestBatch {
        source_event: SourcePublicationEvent {
            stream_id: "resident-test".into(),
            event_id: format!("event-{generation}"),
            expected_base_event_id: base.map(|previous| format!("event-{previous}")),
            payload_sha256: [0; 32],
        },
        repo_id: repo(),
        revision_id: revision(),
        generation: ManifestGeneration::new(generation),
        base_generation: base.map(ManifestGeneration::new),
        manifest_digest: format!("manifest-digest:{generation}"),
        batch_digest: "0".repeat(64),
        mode: if base.is_some() {
            BatchIngestMode::Delta
        } else {
            BatchIngestMode::ReplaceGeneration
        },
        bundle_payload: None,
        clear_surfaces: Vec::new(),
        replace_scopes: scopes,
        tombstone_scopes: Vec::new(),
        semantic_replace_scopes: Vec::new(),
        semantic_tombstone_scopes: Vec::new(),
        seal: true,
    };
    batch.source_event.payload_sha256 = source_event_payload_sha256(&batch)?;
    Ok(batch)
}

fn generation_dir(root: &Path, generation: u64) -> PathBuf {
    GenerationStorageKeyV1::for_repo_revision(&repo(), &revision())
        .generation_dir(root, ManifestGeneration::new(generation))
}

/// `(device, inode)` of one regular file.
type InodeKey = (u64, u64);

/// Bytes of every regular file under `root`, each `(device, inode)` once,
/// skipping `skip_dir` subtrees and the writer lock.
fn inode_set_bytes(
    root: &Path,
    skip_dir: &str,
) -> Result<(u64, BTreeSet<InodeKey>), Box<dyn Error>> {
    let mut seen = BTreeSet::new();
    let mut total = 0_u64;
    let mut pending = vec![root.to_path_buf()];
    while let Some(directory) = pending.pop() {
        for entry in std::fs::read_dir(&directory)? {
            let entry = entry?;
            let name = entry.file_name().to_string_lossy().to_string();
            if name == skip_dir || name.starts_with(".tantivy-writer.lock") {
                continue;
            }
            let file_type = entry.file_type()?;
            if file_type.is_dir() {
                pending.push(entry.path());
            } else if file_type.is_file() {
                let metadata = entry.metadata()?;
                if seen.insert((metadata.dev(), metadata.ino())) {
                    total = total.saturating_add(metadata.len());
                }
            }
        }
    }
    Ok((total, seen))
}

/// The on-disk bytes of the text-authority sidecars.
fn text_authority_disk_bytes(generation: &Path) -> Result<u64, Box<dyn Error>> {
    let mut total = 0_u64;
    for entry in std::fs::read_dir(generation.join(TEXT_AUTHORITY_DIR))? {
        let entry = entry?;
        if entry.file_type()?.is_file() {
            total = total.saturating_add(entry.metadata()?.len());
        }
    }
    Ok(total)
}

/// A body long enough that the decoded folded copy and the expanded
/// postings dominate the CBOR: distinct tokens and trigrams throughout.
fn body(seed: u64) -> String {
    (0..400)
        .map(|index| format!("Token{seed}Alpha{index} beta_{index}_gamma"))
        .collect::<Vec<_>>()
        .join("\n")
}

/// The estimate exceeds the on-disk text-authority bytes plus the mapped
/// index bytes: the decoded authority is counted as what it decodes into,
/// not as its CBOR.
#[test]
fn the_estimate_counts_the_decoded_authority_not_its_cbor() -> TestResult {
    let temp = tempfile::tempdir()?;
    let root = temp.path().join("indexes").join("lexical");
    let adapter = LexicalAdapter::with_state_root(root.clone());
    adapter.build_batch(&batch(
        1,
        None,
        vec![scope("src/a.rs", &body(1))?, scope("src/b.rs", &body(2))?],
    )?)?;
    let dir = generation_dir(&root, 1);
    let (mapped, _inodes) = inode_set_bytes(&dir, TEXT_AUTHORITY_DIR)?;
    let authority_cbor = text_authority_disk_bytes(&dir)?;
    if authority_cbor == 0 {
        return Err("the fixture writes a text authority".into());
    }
    let handle = adapter.open(&repo(), &revision(), ManifestGeneration::new(1))?;
    let estimate = handle.resident_bytes_estimate();
    if estimate <= mapped + authority_cbor {
        return Err(format!(
            "estimate {estimate} does not exceed mapped {mapped} + on-disk authority {authority_cbor}: the decoded authority is being reported as its CBOR"
        )
        .into());
    }
    Ok(())
}

/// A delta generation's estimate counts each mapped inode once.
///
/// The delta hard-links its base's index segments, so the estimate is
/// bounded by the inode-set bytes of its own directory plus its decoded
/// authority, never by a per-link sum.
#[test]
fn a_hard_linked_delta_counts_shared_inodes_once() -> TestResult {
    let temp = tempfile::tempdir()?;
    let root = temp.path().join("indexes").join("lexical");
    let adapter = LexicalAdapter::with_state_root(root.clone());
    adapter.build_batch(&batch(1, None, vec![scope("src/a.rs", &body(1))?])?)?;
    adapter.build_batch(&batch(2, Some(1), vec![scope("src/b.rs", &body(2))?])?)?;
    let base = generation_dir(&root, 1);
    let delta = generation_dir(&root, 2);
    let (_base_bytes, base_inodes) = inode_set_bytes(&base, TEXT_AUTHORITY_DIR)?;
    let (delta_bytes, delta_inodes) = inode_set_bytes(&delta, TEXT_AUTHORITY_DIR)?;
    let shared: BTreeSet<_> = base_inodes.intersection(&delta_inodes).collect();
    if shared.is_empty() {
        return Err(
            "the delta shares no index inode with its base; the fixture must hard-link".into(),
        );
    }
    // The per-link sum would count every hard-linked file as many times as
    // it is linked; the inode set counts it once. The estimate for the
    // delta is at most its inode-set bytes plus a decoded authority that is
    // itself bounded above by a generous multiple of the sidecar bytes.
    let handle = adapter.open(&repo(), &revision(), ManifestGeneration::new(2))?;
    let estimate = handle.resident_bytes_estimate();
    let authority_cbor = text_authority_disk_bytes(&delta)?;
    let decoded_ceiling = authority_cbor.saturating_mul(8);
    if estimate < delta_bytes {
        return Err(
            format!("estimate {estimate} is below the delta's mapped bytes {delta_bytes}").into(),
        );
    }
    if estimate > delta_bytes.saturating_add(decoded_ceiling) {
        return Err(format!(
            "estimate {estimate} exceeds mapped {delta_bytes} + decoded ceiling {decoded_ceiling}; shared inodes are being double counted"
        )
        .into());
    }
    Ok(())
}
