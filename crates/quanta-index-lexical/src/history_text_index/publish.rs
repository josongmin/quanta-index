//! Building one epoch's index and publishing it atomically.
//!
//! The index is built under `e{epoch}.staging`. An incremental build
//! first materializes the base epoch's directories into it: the engine's
//! segment files are immutable across commits, so they are hard-linked
//! (a shared inode is only ever read through), while the two files the
//! engine rewrites in place — its commit file and its managed-file list —
//! are copied, and a live writer's lock files are not inherited at all.
//! This is the same discipline the corpus index applies between a delta
//! generation and its base (§3.4): an epoch's cost is its upserts plus a
//! commit, not the size of the history.
//!
//! Each kind's index is then opened (or created), every upsert deletes
//! the document under its key before adding the new one, the index is
//! committed and its merges awaited, and every segment that lost a row
//! is rewritten without it (`compact_superseded`) so the epoch's scores
//! are a function of its live rows only; the segments an epoch's upserts
//! did not touch stay shared with the base. The manifest is written last,
//! and only then is the staging directory renamed to `e{epoch}` and the
//! rename made durable. A crash anywhere before the rename leaves a
//! staging directory the next attempt removes; a crash after it leaves a
//! complete epoch.

use std::collections::BTreeMap;
use std::path::Path;

use quanta_index_contract::AuxEpochV1;
use quanta_index_core::{
    AuxiliaryGenerationKeyV1, CoreError, HistoryTextBuildV1, HistoryTextDocV1,
    HistoryTextEpochReceiptV1, HistoryTextEpochStatusV1, HistoryTextKindV1,
};
use tantivy::{Index, IndexWriter, TantivyDocument};

use crate::analyzer::register_analyzers;
use crate::history_text_index::layout::{epoch_dir, fsync_parent, kind_dir, staging_dir};
use crate::history_text_index::manifest::{self, HistoryTextManifest};
use crate::history_text_index::schema::{KindSchema, doc_key_text};

/// The engine's commit file, rewritten in place on every commit.
const INDEX_COMMIT_FILE_NAME: &str = "meta.json";
/// The engine's managed-file list, rewritten whenever the file set changes.
const INDEX_MANAGED_FILE_NAME: &str = ".managed.json";
/// Prefix of the engine's lock files, which belong to one live writer only.
const INDEX_LOCK_FILE_PREFIX: &str = ".tantivy";

/// Whether a base-directory entry must be a private copy, not a link.
fn is_epoch_local_entry(file_name: &str) -> bool {
    matches!(file_name, INDEX_COMMIT_FILE_NAME | INDEX_MANAGED_FILE_NAME)
}

/// Whether an entry belongs to a live writer and is not inherited at all.
fn is_writer_lock_entry(file_name: &str) -> bool {
    file_name.starts_with(INDEX_LOCK_FILE_PREFIX)
}

/// Materialize one kind's base directory into `target`: link the
/// immutable entries, copy the epoch-local ones, skip the lock files.
///
/// A link failure is a storage fault, never a reason to fall back to a
/// byte copy and lose the incremental guarantee without saying so.
fn inherit_kind_dir(source: &Path, target: &Path) -> Result<(), CoreError> {
    let entries = std::fs::read_dir(source).map_err(|err| {
        CoreError::Storage(format!(
            "history text index: list base index {}: {err}",
            source.display()
        ))
    })?;
    for entry in entries {
        let entry = entry.map_err(|err| {
            CoreError::Storage(format!(
                "history text index: read base index entry {}: {err}",
                source.display()
            ))
        })?;
        let file_type = entry.file_type().map_err(|err| {
            CoreError::Storage(format!(
                "history text index: inspect base index entry {}: {err}",
                entry.path().display()
            ))
        })?;
        if file_type.is_dir() {
            return Err(CoreError::Storage(format!(
                "history text index: base index {} holds a directory entry",
                source.display()
            )));
        }
        let name = entry.file_name();
        let name = name.to_str().ok_or_else(|| {
            CoreError::Storage(format!(
                "history text index: base index entry has no usable name: {}",
                entry.path().display()
            ))
        })?;
        if is_writer_lock_entry(name) {
            continue;
        }
        let source_path = entry.path();
        let target_path = target.join(name);
        if is_epoch_local_entry(name) {
            let _bytes_copied: u64 = std::fs::copy(&source_path, &target_path).map_err(|err| {
                CoreError::Storage(format!(
                    "history text index: copy epoch-local entry {} -> {}: {err}",
                    source_path.display(),
                    target_path.display()
                ))
            })?;
            continue;
        }
        std::fs::hard_link(&source_path, &target_path).map_err(|err| {
            CoreError::Storage(format!(
                "history text index: link inherited entry {} -> {}: {err}",
                source_path.display(),
                target_path.display()
            ))
        })?;
    }
    Ok(())
}

/// The base epoch an incremental build stands on, proven servable.
fn require_servable_base(
    root: &Path,
    generation: &AuxiliaryGenerationKeyV1,
    base: AuxEpochV1,
) -> Result<std::path::PathBuf, CoreError> {
    let base_dir = epoch_dir(root, generation, base);
    match manifest::epoch_status(&base_dir, base)? {
        HistoryTextEpochStatusV1::Servable => Ok(base_dir),
        HistoryTextEpochStatusV1::Absent => Err(CoreError::Typed {
            code: quanta_index_contract::SearchPlaneErrorCodeV2::HistoryTextIndexNotReady,
            message: format!(
                "history text index: incremental build over epoch {base}, which has no index; a full build is required"
            ),
        }),
        HistoryTextEpochStatusV1::Unsupported { built_with } => Err(CoreError::Typed {
            code:
                quanta_index_contract::SearchPlaneErrorCodeV2::HistoryTextIndexNormalizerUnsupported,
            message: format!(
                "history text index: incremental build over epoch {base}, built under text normalizer {built_with}; a full build is required"
            ),
        }),
    }
}

/// The documents of one build, by kind, one per key (a later upsert of
/// the same key in one build replaces an earlier one, as it does in the
/// row store).
fn documents_by_kind(
    docs: Vec<HistoryTextDocV1>,
) -> BTreeMap<HistoryTextKindV1, BTreeMap<String, HistoryTextDocV1>> {
    let mut by_kind: BTreeMap<HistoryTextKindV1, BTreeMap<String, HistoryTextDocV1>> =
        BTreeMap::new();
    for doc in docs {
        let _replaced = by_kind
            .entry(doc.key.kind())
            .or_default()
            .insert(doc_key_text(&doc.key), doc);
    }
    by_kind
}

fn open_or_create_kind_index(schema: &KindSchema, dir: &Path) -> Result<Index, CoreError> {
    std::fs::create_dir_all(dir).map_err(|err| {
        CoreError::Storage(format!(
            "history text index: create {} index directory {}: {err}",
            schema.kind.as_str(),
            dir.display()
        ))
    })?;
    let directory = tantivy::directory::MmapDirectory::open(dir).map_err(|err| {
        CoreError::Storage(format!(
            "history text index: open {} index directory {}: {err}",
            schema.kind.as_str(),
            dir.display()
        ))
    })?;
    let index = Index::builder()
        .schema(schema.schema.clone())
        .open_or_create(directory)
        .map_err(|err| {
            CoreError::Storage(format!(
                "history text index: open {} index: {err}",
                schema.kind.as_str()
            ))
        })?;
    register_analyzers(&index);
    Ok(index)
}

/// Write one kind's documents into its staged index, commit, and compact
/// away every superseded document (see [`compact_superseded`]).
fn write_kind(
    schema: &KindSchema,
    dir: &Path,
    docs: BTreeMap<String, HistoryTextDocV1>,
    delete_first: bool,
    writer_heap_bytes: usize,
) -> Result<u64, CoreError> {
    let kind = schema.kind.as_str();
    let index = open_or_create_kind_index(schema, dir)?;
    let mut writer = open_writer(&index, kind, writer_heap_bytes)?;
    let mut written = 0_u64;
    for doc in docs.into_values() {
        if delete_first {
            let _opstamp = writer.delete_term(schema.doc_key_term(&doc.key)?);
        }
        let _opstamp = writer.add_document(schema.document(&doc)?).map_err(|err| {
            CoreError::Storage(format!("history text index: add {kind} document: {err}"))
        })?;
        written = written.saturating_add(1);
    }
    let _opstamp = writer.commit().map_err(|err| {
        CoreError::Storage(format!("history text index: commit {kind} index: {err}"))
    })?;
    await_merges(writer, kind)?;
    compact_superseded(&index, kind, writer_heap_bytes)?;
    Ok(written)
}

fn open_writer(
    index: &Index,
    kind: &str,
    writer_heap_bytes: usize,
) -> Result<IndexWriter<TantivyDocument>, CoreError> {
    index
        .writer_with_num_threads(1, writer_heap_bytes)
        .map_err(|err| {
            CoreError::Storage(format!(
                "history text index: open {kind} index writer: {err}"
            ))
        })
}

/// Wait until the engine has no merge in flight or pending for `kind`.
fn await_merges(writer: IndexWriter<TantivyDocument>, kind: &str) -> Result<(), CoreError> {
    writer.wait_merging_threads().map_err(|err| {
        CoreError::Storage(format!(
            "history text index: await {kind} index merges: {err}"
        ))
    })
}

/// The committed segments of `index` that still hold superseded documents.
fn segments_with_superseded_docs(
    index: &Index,
    kind: &str,
) -> Result<Vec<tantivy::SegmentId>, CoreError> {
    Ok(index
        .searchable_segment_metas()
        .map_err(|err| {
            CoreError::Storage(format!(
                "history text index: list {kind} index segments: {err}"
            ))
        })?
        .iter()
        .filter(|meta| meta.num_deleted_docs() > 0)
        .map(tantivy::SegmentMeta::id)
        .collect())
}

/// Rewrite every committed segment that holds a superseded document
/// without it, so the epoch's BM25 statistics count live rows only.
///
/// An upsert of an existing key tombstones the old document in the
/// segment it lives in; the engine keeps counting a tombstoned document
/// in `N`, in `df` and in the field-length total until the segment is
/// merged, which would make a score depend on how the epoch's rows were
/// ingested rather than on what they are (QI-BB-023). Only the segments
/// that lost a row are rewritten — the rest stay hard-linked to the base
/// epoch — and the rewrite is verified: an epoch that still holds a
/// superseded document after it is never published.
fn compact_superseded(
    index: &Index,
    kind: &str,
    writer_heap_bytes: usize,
) -> Result<(), CoreError> {
    let stale = segments_with_superseded_docs(index, kind)?;
    if stale.is_empty() {
        return Ok(());
    }
    let mut writer = open_writer(index, kind, writer_heap_bytes)?;
    let _merged: Option<tantivy::SegmentMeta> = writer.merge(&stale).wait().map_err(|err| {
        CoreError::Storage(format!(
            "history text index: compact superseded {kind} documents: {err}"
        ))
    })?;
    await_merges(writer, kind)?;
    let remaining = segments_with_superseded_docs(index, kind)?;
    if !remaining.is_empty() {
        return Err(CoreError::Storage(format!(
            "history text index: {} {kind} index segment(s) still hold superseded documents after compaction",
            remaining.len()
        )));
    }
    Ok(())
}

/// Build and publish the index of `epoch`; see the module doc.
pub(super) fn publish_epoch(
    root: &Path,
    generation: &AuxiliaryGenerationKeyV1,
    epoch: AuxEpochV1,
    build: HistoryTextBuildV1,
    writer_heap_bytes: usize,
) -> Result<HistoryTextEpochReceiptV1, CoreError> {
    let staging = staging_dir(root, generation, epoch);
    let published = epoch_dir(root, generation, epoch);
    let (base_dir, docs, delete_first) = match build {
        HistoryTextBuildV1::Full { docs } => (None, docs, false),
        HistoryTextBuildV1::Incremental { base, upserts } => {
            if base >= epoch {
                return Err(CoreError::InvalidContract(format!(
                    "history text index: epoch {epoch} cannot be built over epoch {base}, which is not before it"
                )));
            }
            (
                Some(require_servable_base(root, generation, base)?),
                upserts,
                true,
            )
        }
    };
    if staging.exists() {
        std::fs::remove_dir_all(&staging).map_err(|err| {
            CoreError::Storage(format!(
                "history text index: remove stale staging directory {}: {err}",
                staging.display()
            ))
        })?;
    }
    let mut by_kind = documents_by_kind(docs);
    let mut written = 0_u64;
    for kind in [HistoryTextKindV1::Commit, HistoryTextKindV1::Diff] {
        let target = kind_dir(&staging, kind);
        std::fs::create_dir_all(&target).map_err(|err| {
            CoreError::Storage(format!(
                "history text index: create staging directory {}: {err}",
                target.display()
            ))
        })?;
        if let Some(base_dir) = &base_dir {
            inherit_kind_dir(&kind_dir(base_dir, kind), &target)?;
        }
        let schema = KindSchema::build(kind);
        let docs = by_kind.remove(&kind).unwrap_or_default();
        written = written.saturating_add(write_kind(
            &schema,
            &target,
            docs,
            delete_first,
            writer_heap_bytes,
        )?);
    }
    HistoryTextManifest::describe(&staging, epoch)?.write(&staging)?;
    if published.exists() {
        // A leftover of an attempt whose rows never landed; nothing can
        // hold it (see the port contract).
        std::fs::remove_dir_all(&published).map_err(|err| {
            CoreError::Storage(format!(
                "history text index: remove unpublished leftover {}: {err}",
                published.display()
            ))
        })?;
    }
    std::fs::rename(&staging, &published).map_err(|err| {
        CoreError::Storage(format!(
            "history text index: publish {} -> {}: {err}",
            staging.display(),
            published.display()
        ))
    })?;
    fsync_parent(&published)?;
    Ok(HistoryTextEpochReceiptV1 {
        docs_written: written,
    })
}
