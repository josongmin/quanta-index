//! Resident estimates include serving metadata and file views, while authenticated
//! text shard bodies remain transient. Independent source bytes bound retained
//! text copies; an inode-set walk bounds the mapped generation files.

#![forbid(unsafe_code)]

use std::collections::BTreeSet;
use std::error::Error;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};

use quanta_index_contract::lex::LanguageCode;
use quanta_index_contract::{
    BatchIngestMode, ChunkId, ChunkRecord, ManifestGeneration, RepoId, RepoRelativePath,
    RevisionId, SearchCorpusIngestBatch, SearchCorpusReplaceScope,
};
use quanta_index_core::{GenerationStorageKeyV1, LexicalIndexOpenPort, SearchCorpusBatchBuildPort};
use quanta_index_lexical::LexicalAdapter;

#[path = "support/current_source_fixture.rs"]
mod current_source_fixture;

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
    current_source_fixture::text_scope(
        &repo(),
        &revision(),
        path,
        language.clone(),
        body,
        vec![ChunkRecord {
            chunk_id: ChunkId::new(format!("chunk:{path}")),
            repo_relative_path: RepoRelativePath::new(path),
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
    generation: u64,
    base: Option<u64>,
    scopes: Vec<SearchCorpusReplaceScope>,
) -> Result<SearchCorpusIngestBatch, Box<dyn Error>> {
    let mut batch = SearchCorpusIngestBatch {
        source_event: current_source_fixture::empty_event(),
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
    current_source_fixture::finish_batch(&mut batch)?;
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
    skip_dirs: &[&str],
) -> Result<(u64, BTreeSet<InodeKey>), Box<dyn Error>> {
    let mut seen = BTreeSet::new();
    let mut total = 0_u64;
    let mut pending = vec![root.to_path_buf()];
    while let Some(directory) = pending.pop() {
        for entry in std::fs::read_dir(&directory)? {
            let entry = entry?;
            let name = entry.file_name().to_string_lossy().to_string();
            if skip_dirs.contains(&name.as_str()) || name.starts_with(".tantivy-writer.lock") {
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

/// Encoded text postings must not become retained snapshot heap.
#[test]
fn the_estimate_excludes_transient_text_postings_and_counts_serving_metadata() -> TestResult {
    let temp = tempfile::tempdir()?;
    let root = temp.path().join("indexes").join("lexical");
    let adapter = LexicalAdapter::with_state_root(root.clone());
    let _stages = adapter.build_batch(&batch(
        1,
        None,
        vec![scope("src/a.rs", &body(1))?, scope("src/b.rs", &body(2))?],
    )?)?;
    let dir = generation_dir(&root, 1);
    let (mapped, _inodes) = inode_set_bytes(&dir, &[TEXT_AUTHORITY_DIR, "file-authority"])?;
    let authority_cbor = text_authority_disk_bytes(&dir)?;
    if authority_cbor == 0 {
        return Err("the fixture writes a text authority".into());
    }
    let handle = adapter.open(
        &repo(),
        &revision(),
        ManifestGeneration::new(1),
        &quanta_index_core::RequestBudgetV1::unbounded(),
    )?;
    let estimate = handle.resident_bytes_estimate();
    let source_bytes = u64::try_from(
        body(1)
            .len()
            .checked_add(body(2).len())
            .ok_or("fixture source length overflow")?,
    )?;
    // The supported full-file view has original, NFC and folded copies.
    // All other metadata for this two-file fixture fits a fixed 64 KiB budget.
    let ceiling = source_bytes
        .checked_mul(3)
        .and_then(|bytes| bytes.checked_add(mapped))
        .and_then(|bytes| bytes.checked_add(65_536))
        .ok_or("fixture serving ceiling overflow")?;
    if estimate < mapped || estimate > ceiling {
        return Err(format!("estimate {estimate} is outside mapped {mapped} .. serving ceiling {ceiling}; transient text postings must not remain resident").into());
    }
    Ok(())
}

/// A delta generation's estimate counts each mapped inode once.
///
/// The delta hard-links its base's index segments, so the estimate is
/// bounded by its mapped inode set plus source-file views and metadata.
#[test]
fn a_hard_linked_delta_counts_shared_inodes_once() -> TestResult {
    let temp = tempfile::tempdir()?;
    let root = temp.path().join("indexes").join("lexical");
    let adapter = LexicalAdapter::with_state_root(root.clone());
    let _stages = adapter.build_batch(&batch(1, None, vec![scope("src/a.rs", &body(1))?])?)?;
    let _stages = adapter.build_batch(&batch(2, Some(1), vec![scope("src/b.rs", &body(2))?])?)?;
    let base = generation_dir(&root, 1);
    let delta = generation_dir(&root, 2);
    let (_base_bytes, base_inodes) =
        inode_set_bytes(&base, &[TEXT_AUTHORITY_DIR, "file-authority"])?;
    let (delta_bytes, delta_inodes) =
        inode_set_bytes(&delta, &[TEXT_AUTHORITY_DIR, "file-authority"])?;
    let shared: BTreeSet<_> = base_inodes.intersection(&delta_inodes).collect();
    if shared.is_empty() {
        return Err(
            "the delta shares no index inode with its base; the fixture must hard-link".into(),
        );
    }
    // The inode set counts each shared mapped file once. Only source-file
    // views and bounded metadata are added; text postings remain transient.
    let handle = adapter.open(
        &repo(),
        &revision(),
        ManifestGeneration::new(2),
        &quanta_index_core::RequestBudgetV1::unbounded(),
    )?;
    let estimate = handle.resident_bytes_estimate();
    let serving_ceiling = u64::try_from(
        body(1)
            .len()
            .checked_add(body(2).len())
            .ok_or("fixture source length overflow")?,
    )?
    .checked_mul(3)
    .and_then(|bytes| bytes.checked_add(65_536))
    .ok_or("fixture serving ceiling overflow")?;
    if estimate < delta_bytes {
        return Err(
            format!("estimate {estimate} is below the delta's mapped bytes {delta_bytes}").into(),
        );
    }
    if estimate > delta_bytes.saturating_add(serving_ceiling) {
        return Err(format!(
            "estimate {estimate} exceeds mapped {delta_bytes} + decoded ceiling {serving_ceiling}; shared inodes are being double counted"
        )
        .into());
    }
    Ok(())
}

/// Public leaf execution keeps fixed source answers and a stable resident charge.
#[test]
#[expect(
    clippy::panic_in_result_fn,
    reason = "fixed public query and old-pin regression assertions"
)]
fn transient_text_queries_keep_fixed_answers_and_old_generation_pin() -> TestResult {
    use quanta_index_contract::{LQ_VERSION_TAG, LqExpr, LqLeaf, LqOptions, LqQuery, LqSpan};
    use quanta_index_core::RequestBudgetV1;
    let temp = tempfile::tempdir()?;
    let root = temp.path().join("indexes/lexical");
    let adapter = LexicalAdapter::with_state_root(root);
    let _built = adapter.build_batch(&batch(
        1,
        None,
        vec![
            scope("a.rs", "alpha beta")?,
            scope("b.rs", "alpha gamma")?,
            scope("c.rs", "gamma delta")?,
        ],
    )?)?;
    let old = adapter.open(
        &repo(),
        &revision(),
        ManifestGeneration::new(1),
        &RequestBudgetV1::unbounded(),
    )?;
    let resident = old.resident_bytes_estimate();
    let _delta = adapter.build_batch(&batch(2, Some(1), vec![scope("a.rs", "retired token")?])?)?;
    let current = adapter.open(
        &repo(),
        &revision(),
        ManifestGeneration::new(2),
        &RequestBudgetV1::unbounded(),
    )?;
    for (leaf, expected) in [
        (
            LqLeaf::RawString("alpha".into()),
            vec!["chunk:a.rs", "chunk:b.rs"],
        ),
        (LqLeaf::Phrase("alpha beta".into()), vec!["chunk:a.rs"]),
        (
            LqLeaf::Regex("alpha|delta".into()),
            vec!["chunk:a.rs", "chunk:b.rs", "chunk:c.rs"],
        ),
        (
            LqLeaf::Regex(".*".into()),
            vec!["chunk:a.rs", "chunk:b.rs", "chunk:c.rs"],
        ),
    ] {
        let query = LqQuery {
            lq_version: LQ_VERSION_TAG,
            expr: LqExpr::Leaf(leaf),
            filters: Vec::new(),
            options: LqOptions::defaults(),
            directives: Vec::new(),
            source_span: LqSpan::eof(0),
        };
        let actual = old.search(&query, 10, &RequestBudgetV1::unbounded())?;
        assert_eq!(
            actual
                .iter()
                .map(|row| row.candidate_id.as_str())
                .collect::<BTreeSet<_>>(),
            expected.into_iter().collect()
        );
        assert_eq!(
            old.resident_bytes_estimate(),
            resident,
            "query retained decoded text shards"
        );
        if matches!(query.expr, LqExpr::Leaf(LqLeaf::RawString(_))) {
            let actual = current.search(&query, 10, &RequestBudgetV1::unbounded())?;
            assert_eq!(
                actual
                    .iter()
                    .map(|row| row.candidate_id.as_str())
                    .collect::<BTreeSet<_>>(),
                BTreeSet::from(["chunk:b.rs"])
            );
        }
    }
    Ok(())
}
