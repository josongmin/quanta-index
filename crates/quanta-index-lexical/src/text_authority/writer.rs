//! Publishing a text authority: the touched shards and the manifest.
//!
//! Two entry points share one publish step. [`rebuild`] derives every
//! shard from the live documents; [`update`] loads only the shards a delta
//! touched, carries their untouched documents as postings, retires and
//! adds the delta's documents, and lists every other shard unchanged. The
//! publish step writes each built shard under its content-digest name by
//! atomic durable rename, then the manifest the same way, then removes
//! whatever the manifest does not own — a superseded shard, a crash's
//! leftover — so the directory is exactly the manifest's file set.
//!
//! Write bytes are proportional to the touched shards, not to the corpus:
//! an untouched shard is never read, serialized or written, and in a delta
//! generation its file is the hard link the base clone made.

use std::collections::{BTreeMap, BTreeSet};
use std::fs::File;
use std::path::Path;
use std::time::Instant;

use quanta_index_contract::ManifestGeneration;
use quanta_index_core::CoreError;

use crate::text_authority::manifest::{
    MAX_DOC_ID, ShardEntry, TEXT_AUTHORITY_DIR_NAME, TEXT_AUTHORITY_MANIFEST_FILE_NAME,
    TextAuthorityManifest, manifest_path, read_manifest, shard_index_of, text_authority_dir,
};
use crate::text_authority::reader::load_shard;
use crate::text_authority::shard::{ShardBody, ShardBuilders, sha256_of_bytes};

/// What one publish derived, retired, wrote and carried (QI-BB-006).
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) struct TextAuthorityWriteReceipt {
    /// Documents tokenized and posted.
    pub(crate) docs_derived: u64,
    /// Documents removed from inherited shards.
    pub(crate) docs_retired: u64,
    /// Shard files written.
    pub(crate) shards_written: u64,
    /// Shards listed unchanged: their files were not written by this publish.
    pub(crate) shards_inherited: u64,
}

/// One canonical write result; timings describe completed, non-overlapping work.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct TextAuthorityWriteResult {
    pub(crate) receipt: TextAuthorityWriteReceipt,
    /// Includes loading and validating touched prior shards for a delta.
    pub(crate) shard_build_ns: u64,
    /// Includes encoding, durable writes, and removal of unowned files.
    pub(crate) publish_ns: u64,
}

/// One document a batch added, with the doc id the index stores for it.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct AddedTextDoc {
    pub(crate) doc_id: u64,
    pub(crate) candidate_id: String,
    pub(crate) text: String,
}

/// What the publish step does with one shard index.
enum ShardOutcome {
    /// Listed as the prior manifest listed it; the file stays as it is.
    Inherit(ShardEntry),
    /// Built by this publish; written unless the prior manifest already
    /// listed this exact content.
    Built(ShardBody),
}

/// Derive every shard from `docs`, the live text documents of the index.
///
/// `max_doc_id` is the watermark after this batch's allocations; every
/// document's id must be at or below it. A shard whose content equals what
/// `prior` listed is not written again.
pub(crate) fn rebuild(
    generation_dir: &Path,
    generation: ManifestGeneration,
    docs: Vec<AddedTextDoc>,
    prior: Option<&TextAuthorityManifest>,
    max_doc_id: u64,
) -> Result<TextAuthorityWriteResult, CoreError> {
    let shard_started = Instant::now();
    let docs_derived = count(docs.len(), "text authority doc count")?;
    let mut by_shard: BTreeMap<u64, Vec<AddedTextDoc>> = BTreeMap::new();
    let mut seen: BTreeSet<u64> = BTreeSet::new();
    for doc in docs {
        ensure_doc_id(doc.doc_id, max_doc_id)?;
        if !seen.insert(doc.doc_id) {
            return Err(CoreError::InvalidContract(format!(
                "lexical: text authority rebuild saw doc id {} twice",
                doc.doc_id
            )));
        }
        by_shard
            .entry(shard_index_of(doc.doc_id))
            .or_default()
            .push(doc);
    }
    let mut outcomes: BTreeMap<u64, ShardOutcome> = BTreeMap::new();
    for (index, docs) in by_shard {
        let mut builders = ShardBuilders::empty(index, generation)?;
        for doc in docs {
            builders.upsert(doc.doc_id, doc.candidate_id, &doc.text)?;
        }
        let _prior = outcomes.insert(index, ShardOutcome::Built(builders.finish()?));
    }
    let shard_build_ns = crate::adapter_ingest::elapsed_stage_ns(shard_started)?;
    let publish_started = Instant::now();
    let (shards_written, shards_inherited) = publish(generation_dir, prior, outcomes, max_doc_id)?;
    Ok(TextAuthorityWriteResult {
        receipt: TextAuthorityWriteReceipt {
            docs_derived,
            docs_retired: 0,
            shards_written,
            shards_inherited,
        },
        shard_build_ns,
        publish_ns: crate::adapter_ingest::elapsed_stage_ns(publish_started)?,
    })
}

/// Apply a scope delta to the prior shards: retire `retired`, add `added`.
///
/// Only the shards holding a retired or added document are read and built;
/// `touched` is the planner's expectation of that set and must match, so a
/// disagreement between the plan and the allocation is an error rather
/// than a shard silently left out. A retired doc id the index names but
/// its shard does not hold is a corrupt authority, never a no-op.
pub(crate) fn update(
    generation_dir: &Path,
    generation: ManifestGeneration,
    prior: &TextAuthorityManifest,
    retired: &BTreeMap<u64, String>,
    added: &[AddedTextDoc],
    touched: &BTreeSet<u64>,
    max_doc_id: u64,
) -> Result<TextAuthorityWriteResult, CoreError> {
    let shard_started = Instant::now();
    let mut actual: BTreeSet<u64> = retired
        .keys()
        .map(|doc_id| shard_index_of(*doc_id))
        .collect();
    for doc in added {
        ensure_doc_id(doc.doc_id, max_doc_id)?;
        if doc.doc_id <= prior.max_doc_id {
            return Err(CoreError::InvalidContract(format!(
                "lexical: text authority delta adds doc id {} at or below the prior watermark {}",
                doc.doc_id, prior.max_doc_id
            )));
        }
        let _inserted = actual.insert(shard_index_of(doc.doc_id));
    }
    if actual != *touched {
        return Err(CoreError::InvalidContract(format!(
            "lexical: text authority delta planned shards {touched:?} but touches {actual:?}"
        )));
    }
    let mut outcomes: BTreeMap<u64, ShardOutcome> = BTreeMap::new();
    for entry in &prior.shards {
        if !actual.contains(&entry.index) {
            let _prior = outcomes.insert(entry.index, ShardOutcome::Inherit(entry.clone()));
        }
    }
    for index in &actual {
        let mut builders = match prior.shard(*index) {
            Some(entry) => {
                ShardBuilders::from_prior(*index, generation, load_shard(generation_dir, entry)?)?
            }
            None => ShardBuilders::empty(*index, generation)?,
        };
        for (doc_id, candidate_id) in retired {
            if shard_index_of(*doc_id) != *index {
                continue;
            }
            if !builders.retire(*doc_id)? {
                return Err(crate::index_store::sidecar_corrupt(
                    generation_dir,
                    &format!("{TEXT_AUTHORITY_DIR_NAME}/shard {index}"),
                    &format!(
                        "the index names doc {doc_id} ({candidate_id}) for retirement but the shard does not hold it"
                    ),
                ));
            }
        }
        for doc in added {
            if shard_index_of(doc.doc_id) != *index {
                continue;
            }
            builders.upsert(doc.doc_id, doc.candidate_id.clone(), &doc.text)?;
        }
        let _prior = outcomes.insert(*index, ShardOutcome::Built(builders.finish()?));
    }
    let shard_build_ns = crate::adapter_ingest::elapsed_stage_ns(shard_started)?;
    let publish_started = Instant::now();
    let (shards_written, shards_inherited) =
        publish(generation_dir, Some(prior), outcomes, max_doc_id)?;
    Ok(TextAuthorityWriteResult {
        receipt: TextAuthorityWriteReceipt {
            docs_derived: count(added.len(), "text authority add count")?,
            docs_retired: count(retired.len(), "text authority retire count")?,
            shards_written,
            shards_inherited,
        },
        shard_build_ns,
        publish_ns: crate::adapter_ingest::elapsed_stage_ns(publish_started)?,
    })
}

/// Bring the directory to the manifest's file set before the seal measures
/// it, and hand the seal the manifest it will commit to.
///
/// `Ok(None)` is a generation without a text authority. A directory
/// without a manifest is neither: it is refused, so the seal never commits
/// to a half-published authority.
pub(crate) fn finalize_for_seal(
    generation_dir: &Path,
) -> Result<Option<TextAuthorityManifest>, CoreError> {
    let dir = text_authority_dir(generation_dir);
    match crate::sealed_generation::optional_entry_metadata(&dir) {
        Ok(None) => return Ok(None),
        Ok(Some(metadata)) if metadata.is_dir() => {}
        Ok(Some(_)) => {
            return Err(CoreError::Storage(format!(
                "lexical: refusing to seal {}: text-authority is not a directory",
                generation_dir.display()
            )));
        }
        Err(error) => {
            return Err(CoreError::Storage(format!(
                "lexical: inspect text authority {} before sealing: {error}",
                dir.display()
            )));
        }
    }
    let Some(manifest) = read_manifest(generation_dir)? else {
        return Err(CoreError::Storage(format!(
            "lexical: refusing to seal {}: a text-authority directory without a manifest",
            generation_dir.display()
        )));
    };
    remove_unowned_entries(generation_dir, &manifest)?;
    Ok(Some(manifest))
}

fn count(value: usize, what: &str) -> Result<u64, CoreError> {
    u64::try_from(value)
        .map_err(|err| CoreError::InvalidContract(format!("lexical: {what} overflow: {err}")))
}

fn ensure_doc_id(doc_id: u64, max_doc_id: u64) -> Result<(), CoreError> {
    if doc_id == 0 || doc_id > MAX_DOC_ID {
        return Err(CoreError::InvalidContract(format!(
            "lexical: text authority doc id {doc_id} is outside 1..={MAX_DOC_ID}"
        )));
    }
    if doc_id > max_doc_id {
        return Err(CoreError::InvalidContract(format!(
            "lexical: text authority doc id {doc_id} is above the watermark {max_doc_id}"
        )));
    }
    Ok(())
}

/// Write the built shards and the manifest, then drop what the manifest
/// does not own. Returns `(shards written, shards inherited)`.
fn publish(
    generation_dir: &Path,
    prior: Option<&TextAuthorityManifest>,
    outcomes: BTreeMap<u64, ShardOutcome>,
    max_doc_id: u64,
) -> Result<(u64, u64), CoreError> {
    let dir = text_authority_dir(generation_dir);
    std::fs::create_dir_all(&dir).map_err(|err| {
        CoreError::Storage(format!(
            "lexical: create text authority directory {}: {err}",
            dir.display()
        ))
    })?;
    fsync_directory(generation_dir)?;
    let mut shards: Vec<ShardEntry> = Vec::with_capacity(outcomes.len());
    let mut written = 0_u64;
    let mut inherited = 0_u64;
    for (index, outcome) in outcomes {
        match outcome {
            ShardOutcome::Inherit(entry) => {
                if !entry.path(generation_dir).is_file() {
                    return Err(crate::index_store::sidecar_corrupt(
                        generation_dir,
                        &format!("{TEXT_AUTHORITY_DIR_NAME}/{}", entry.file_name()),
                        "missing",
                    ));
                }
                shards.push(entry);
                inherited = inherited.saturating_add(1);
            }
            ShardOutcome::Built(body) => {
                let Some((min_doc_id, max_shard_doc_id)) = body.doc_id_extremes() else {
                    // Every document of the shard was retired: it is dropped
                    // from the manifest and its file becomes unowned.
                    continue;
                };
                let bytes = body.encode()?;
                let entry = ShardEntry {
                    index,
                    rows: body.rows()?,
                    min_doc_id,
                    max_doc_id: max_shard_doc_id,
                    bytes: count(bytes.len(), "text authority shard length")?,
                    sha256: sha256_of_bytes(&bytes),
                };
                let path = entry.path(generation_dir);
                let already_listed = prior
                    .and_then(|manifest| manifest.shard(index))
                    .is_some_and(|listed| listed.sha256 == entry.sha256 && path.is_file());
                if already_listed {
                    inherited = inherited.saturating_add(1);
                } else {
                    crate::index_store::write_atomic_durable(
                        &path,
                        &bytes,
                        "text authority shard",
                    )?;
                    written = written.saturating_add(1);
                }
                shards.push(entry);
            }
        }
    }
    let manifest = TextAuthorityManifest { max_doc_id, shards };
    let bytes = manifest.encode()?;
    crate::index_store::write_atomic_durable(
        &manifest_path(generation_dir),
        &bytes,
        "text authority manifest",
    )?;
    remove_unowned_entries(generation_dir, &manifest)?;
    Ok((written, inherited))
}

/// Remove every entry of the text-authority directory the manifest does
/// not own: superseded shard versions and crash leftovers.
///
/// Unlinking a hard link never touches the base generation's inode, so a
/// superseded shard the delta inherited is dropped from this directory
/// only.
fn remove_unowned_entries(
    generation_dir: &Path,
    manifest: &TextAuthorityManifest,
) -> Result<(), CoreError> {
    let dir = text_authority_dir(generation_dir);
    let mut owned: BTreeSet<String> = manifest.shards.iter().map(ShardEntry::file_name).collect();
    let _inserted = owned.insert(TEXT_AUTHORITY_MANIFEST_FILE_NAME.to_string());
    let mut removed_any = false;
    for entry in std::fs::read_dir(&dir).map_err(|err| {
        CoreError::Storage(format!(
            "lexical: list text authority directory {}: {err}",
            dir.display()
        ))
    })? {
        let entry = entry.map_err(|err| {
            CoreError::Storage(format!(
                "lexical: read text authority entry in {}: {err}",
                dir.display()
            ))
        })?;
        let name = entry.file_name().to_string_lossy().into_owned();
        if owned.contains(&name) {
            continue;
        }
        let file_type = entry.file_type().map_err(|err| {
            CoreError::Storage(format!(
                "lexical: inspect text authority entry {}: {err}",
                entry.path().display()
            ))
        })?;
        if !file_type.is_file() {
            return Err(CoreError::Storage(format!(
                "lexical: text authority directory holds a non-file entry {}",
                entry.path().display()
            )));
        }
        std::fs::remove_file(entry.path()).map_err(|err| {
            CoreError::Storage(format!(
                "lexical: remove unowned text authority entry {}: {err}",
                entry.path().display()
            ))
        })?;
        removed_any = true;
    }
    if removed_any {
        fsync_directory(&dir)?;
    }
    Ok(())
}

fn fsync_directory(dir: &Path) -> Result<(), CoreError> {
    File::open(dir)
        .and_then(|directory| directory.sync_all())
        .map_err(|error| {
            CoreError::Storage(format!(
                "lexical: fsync directory {}: {error}",
                dir.display()
            ))
        })
}
