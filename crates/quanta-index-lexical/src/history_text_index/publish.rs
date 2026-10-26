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
use std::sync::Arc;

use quanta_index_contract::AuxEpochV1;
use quanta_index_core::HistoryTextFileIoPort;
use quanta_index_core::{
    AuxiliaryGenerationKeyV1, CoreError, HistoryTextBuildV1, HistoryTextDocKeyV1, HistoryTextDocV1,
    HistoryTextEpochReceiptV1, HistoryTextEpochStatusV1, HistoryTextKindV1, RequestBudgetV1,
};
use sha2::{Digest as _, Sha256};
use tantivy::{Index, IndexWriter, TantivyDocument, TantivyError};

use crate::analyzer::register_analyzers;
use crate::history_text_index::layout::sync_parent_admitted;
use crate::history_text_index::layout::{epoch_dir, fsync_parent_observed, kind_dir, staging_dir};
use crate::history_text_index::manifest::{self, HistoryTextManifest};
use crate::history_text_index::physical_directory::{PhysicalDirectory, engine_error};
use crate::history_text_index::schema::{KindSchema, doc_key_text};

/// The engine's commit file, rewritten in place on every commit.
const INDEX_COMMIT_FILE_NAME: &str = "meta.json";
/// The engine's managed-file list, rewritten whenever the file set changes.
const INDEX_MANAGED_FILE_NAME: &str = ".managed.json";
/// Prefix of the engine's lock files, which belong to one live writer only.
const INDEX_LOCK_FILE_PREFIX: &str = ".tantivy";

fn digest_width(hasher: &mut Sha256, length: usize) -> Result<(), CoreError> {
    let length = u64::try_from(length).map_err(|error| {
        CoreError::InvalidContract(format!("history build field width exceeds u64: {error}"))
    })?;
    hasher.update(length.to_be_bytes());
    Ok(())
}

fn digest_bytes(hasher: &mut Sha256, bytes: &[u8]) -> Result<(), CoreError> {
    digest_width(hasher, bytes.len())?;
    hasher.update(bytes);
    Ok(())
}

/// Hash the exact caller-supplied sequence, including duplicate keys and order.
/// A retry cannot silently replace a published epoch with a different build.
fn build_sha256(build: &HistoryTextBuildV1) -> Result<[u8; 32], CoreError> {
    let mut hasher = Sha256::new();
    hasher.update(b"quanta-history-text-build-v1\0");
    let docs = match build {
        HistoryTextBuildV1::Full { docs } => {
            hasher.update([0]);
            docs
        }
        HistoryTextBuildV1::Incremental { base, upserts } => {
            hasher.update([1]);
            hasher.update(base.get().to_be_bytes());
            upserts
        }
    };
    digest_width(&mut hasher, docs.len())?;
    for doc in docs {
        match &doc.key {
            HistoryTextDocKeyV1::Commit { sha } => {
                hasher.update([0]);
                hasher.update(sha.as_bytes());
            }
            HistoryTextDocKeyV1::Diff { sha, file_path } => {
                hasher.update([1]);
                hasher.update(sha.as_bytes());
                digest_bytes(&mut hasher, file_path.as_bytes())?;
            }
        }
        hasher.update(doc.committer_time_ms.to_be_bytes());
        digest_bytes(&mut hasher, doc.text.as_bytes())?;
    }
    Ok(hasher.finalize().into())
}

fn replay_receipt(
    manifest: &HistoryTextManifest,
    expected_build: [u8; 32],
) -> Result<HistoryTextEpochReceiptV1, CoreError> {
    if manifest.build_sha256 != expected_build {
        return Err(CoreError::InvalidContract(
            "history text index: published epoch has a different original build".into(),
        ));
    }
    Ok(HistoryTextEpochReceiptV1 {
        docs_written: manifest.docs_written,
    })
}

fn replay_published(
    published: &Path,
    epoch: AuxEpochV1,
    expected_build: [u8; 32],
    io: Option<&dyn HistoryTextFileIoPort>,
) -> Result<HistoryTextEpochReceiptV1, CoreError> {
    let status = if let Some(io) = io {
        manifest::epoch_status_admitted(published, epoch, io)?
    } else {
        manifest::epoch_status(published, epoch)?
    };
    if status != HistoryTextEpochStatusV1::Servable {
        return Err(CoreError::InvalidContract(format!(
            "history text index: published epoch {epoch} is not servable for replay: {status:?}"
        )));
    }
    let manifest = if let Some(io) = io {
        HistoryTextManifest::read_admitted(published, io)?
    } else {
        HistoryTextManifest::read(published)?
    };
    replay_receipt(&manifest, expected_build)
}

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
fn inherit_kind_dir(
    source: &Path,
    target: &Path,
    budget: &RequestBudgetV1,
) -> Result<(), CoreError> {
    let entries = std::fs::read_dir(source).map_err(|err| {
        CoreError::Storage(format!(
            "history text index: list base index {}: {err}",
            source.display()
        ))
    })?;
    for entry in entries {
        budget.checkpoint("history text: inherit base entry")?;
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

fn inherit_kind_dir_admitted(
    source: &Path,
    target: &Path,
    io: &dyn HistoryTextFileIoPort,
    budget: &RequestBudgetV1,
) -> Result<(), CoreError> {
    for entry in io.list_directory(source)? {
        budget.checkpoint("history text: inherit admitted base entry")?;
        if entry.metadata.is_dir() {
            return Err(CoreError::Storage(format!(
                "history text index: base index {} holds a directory entry",
                source.display()
            )));
        }
        let name = entry.name.to_str().ok_or_else(|| {
            CoreError::Storage(format!(
                "history text index: base index entry has no usable name in {}",
                source.display()
            ))
        })?;
        if is_writer_lock_entry(name) {
            continue;
        }
        let source_path = source.join(&entry.name);
        let target_path = target.join(&entry.name);
        if is_epoch_local_entry(name) {
            io.copy_file(&source_path, &target_path)?;
        } else {
            io.link_file(&source_path, &target_path)?;
        }
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

fn require_servable_base_admitted(
    root: &Path,
    generation: &AuxiliaryGenerationKeyV1,
    base: AuxEpochV1,
    io: &dyn HistoryTextFileIoPort,
) -> Result<std::path::PathBuf, CoreError> {
    let base_dir = epoch_dir(root, generation, base);
    match manifest::epoch_status_admitted(&base_dir, base, io)? {
        HistoryTextEpochStatusV1::Servable => Ok(base_dir),
        HistoryTextEpochStatusV1::Absent => Err(CoreError::Typed {
            code: quanta_index_contract::SearchPlaneErrorCodeV2::HistoryTextIndexNotReady,
            message: format!("history text index: incremental base epoch {base} is absent"),
        }),
        HistoryTextEpochStatusV1::Unsupported { built_with } => Err(CoreError::Typed {
            code:
                quanta_index_contract::SearchPlaneErrorCodeV2::HistoryTextIndexNormalizerUnsupported,
            message: format!(
                "history text index: incremental base epoch {base} uses unsupported normalizer {built_with}"
            ),
        }),
    }
}

/// The documents of one build, by kind, one per key (a later upsert of
/// the same key in one build replaces an earlier one, as it does in the
/// row store).
fn documents_by_kind(
    docs: Vec<HistoryTextDocV1>,
    budget: &RequestBudgetV1,
) -> Result<BTreeMap<HistoryTextKindV1, BTreeMap<String, HistoryTextDocV1>>, CoreError> {
    let mut by_kind: BTreeMap<HistoryTextKindV1, BTreeMap<String, HistoryTextDocV1>> =
        BTreeMap::new();
    for doc in docs {
        budget.checkpoint("history text: group document")?;
        let _replaced = by_kind
            .entry(doc.key.kind())
            .or_default()
            .insert(doc_key_text(&doc.key), doc);
    }
    Ok(by_kind)
}

fn open_or_create_kind_index(
    schema: &KindSchema,
    dir: &Path,
    io: Option<&Arc<dyn HistoryTextFileIoPort>>,
    budget: &RequestBudgetV1,
) -> Result<Index, CoreError> {
    budget.checkpoint("history text: open or create kind index")?;
    if let Some(admission) = io {
        admission.ensure_directory(dir)?;
    } else {
        std::fs::create_dir_all(dir).map_err(|err| {
            CoreError::Storage(format!(
                "history text index: create {} index directory {}: {err}",
                schema.kind.as_str(),
                dir.display()
            ))
        })?;
    }
    let directory: Box<dyn tantivy::directory::Directory> = if let Some(admission) = io {
        Box::new(PhysicalDirectory::new(dir, Arc::clone(admission)))
    } else {
        Box::new(tantivy::directory::MmapDirectory::open(dir).map_err(|err| {
            CoreError::Storage(format!(
                "history text index: open {} index directory {}: {err}",
                schema.kind.as_str(),
                dir.display()
            ))
        })?)
    };
    let index = Index::builder()
        .schema(schema.schema.clone())
        .open_or_create(directory)
        .map_err(|err| {
            let context = format!("history text index: open {} index", schema.kind.as_str());
            if io.is_some() {
                engine_error(&context, &err)
            } else {
                CoreError::Storage(format!("{context}: {err}"))
            }
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
    io: Option<&Arc<dyn HistoryTextFileIoPort>>,
    budget: &RequestBudgetV1,
) -> Result<u64, CoreError> {
    let kind = schema.kind.as_str();
    let index = open_or_create_kind_index(schema, dir, io, budget)?;
    budget.checkpoint("history text: before writer open")?;
    let mut writer = open_writer(&index, kind, writer_heap_bytes, io.is_some())?;
    let mut written = 0_u64;
    let mut erased_worker_cause = false;
    let work = (|| -> Result<(), CoreError> {
        for doc in docs.into_values() {
            budget.checkpoint("history text: before document upsert")?;
            if delete_first {
                let _opstamp = writer.delete_term(schema.doc_key_term(&doc.key)?);
            }
            let _opstamp = writer.add_document(schema.document(&doc)?).map_err(|err| {
                erased_worker_cause =
                    matches!(err, TantivyError::ErrorInThread(_) | TantivyError::Poisoned);
                let context = format!("history text index: add {kind} document");
                if io.is_some() {
                    engine_error(&context, &err)
                } else {
                    CoreError::Storage(format!("{context}: {err}"))
                }
            })?;
            written = written.saturating_add(1);
        }
        budget.checkpoint("history text: before commit")?;
        let _opstamp = writer.commit().map_err(|err| {
            erased_worker_cause =
                matches!(err, TantivyError::ErrorInThread(_) | TantivyError::Poisoned);
            let context = format!("history text index: commit {kind} index");
            if io.is_some() {
                engine_error(&context, &err)
            } else {
                CoreError::Storage(format!("{context}: {err}"))
            }
        })?;
        budget.checkpoint("history text: before merge wait")
    })();
    // Tantivy Drop joins indexing workers but does not wait for merge threads.
    // Drain the original writer even when a checkpoint or document operation
    // failed, before a retry may delete this staging tree.
    let drained = await_merges(writer, kind, io.is_some());
    let work = restore_worker_interruption(work, erased_worker_cause, io);
    let after = budget.checkpoint("history text: after writer merge drain");
    let work = original_after_drain(work, after, "post-drain request budget");
    original_after_drain(work, drained, "history writer merge drain")?;
    compact_superseded(&index, kind, writer_heap_bytes, io, budget)?;
    Ok(written)
}

fn open_writer(
    index: &Index,
    kind: &str,
    writer_heap_bytes: usize,
    admitted: bool,
) -> Result<IndexWriter<TantivyDocument>, CoreError> {
    index
        .writer_with_num_threads(1, writer_heap_bytes)
        .map_err(|err| {
            let context = format!("history text index: open {kind} index writer");
            if admitted {
                engine_error(&context, &err)
            } else {
                CoreError::Storage(format!("{context}: {err}"))
            }
        })
}

/// Wait until the engine has no merge in flight or pending for `kind`.
fn await_merges(
    writer: IndexWriter<TantivyDocument>,
    kind: &str,
    admitted: bool,
) -> Result<(), CoreError> {
    writer.wait_merging_threads().map_err(|err| {
        let context = format!("history text index: await {kind} index merges");
        if admitted {
            engine_error(&context, &err)
        } else {
            CoreError::Storage(format!("{context}: {err}"))
        }
    })
}

fn original_after_drain(
    work: Result<(), CoreError>,
    drained: Result<(), CoreError>,
    context: &str,
) -> Result<(), CoreError> {
    match (work, drained) {
        (Ok(()), Ok(())) => Ok(()),
        (Err(primary), Ok(())) | (Ok(()), Err(primary)) => Err(primary),
        (Err(mut primary), Err(drain)) => {
            let message = match &mut primary {
                CoreError::InvalidContract(message)
                | CoreError::NotReady(message)
                | CoreError::NotImplemented(message)
                | CoreError::NotFound(message)
                | CoreError::Storage(message)
                | CoreError::StorageIo { message, .. }
                | CoreError::Typed { message, .. } => message,
            };
            message.push_str("; ");
            message.push_str(context);
            message.push_str(" also failed: ");
            message.push_str(&drain.to_string());
            Err(primary)
        }
    }
}

fn restore_worker_interruption(
    work: Result<(), CoreError>,
    erased_worker_cause: bool,
    io: Option<&Arc<dyn HistoryTextFileIoPort>>,
) -> Result<(), CoreError> {
    let Err(primary) = work else {
        return Ok(());
    };
    let Some(io) = io.filter(|_io| erased_worker_cause) else {
        return Err(primary);
    };
    restore_observed_worker_interruption(primary, io.original_interruption())
}

fn restore_observed_worker_interruption(
    primary: CoreError,
    observed: Result<Option<CoreError>, CoreError>,
) -> Result<(), CoreError> {
    match observed {
        Ok(Some(original)) => original_after_drain(
            Err(original),
            Err(primary),
            "engine worker source was erased",
        ),
        Ok(None) => Err(primary),
        Err(witness) => {
            original_after_drain(Err(primary), Err(witness), "original interruption witness")
        }
    }
}

/// The committed segments of `index` that still hold superseded documents.
fn segments_with_superseded_docs(
    index: &Index,
    kind: &str,
    admitted: bool,
) -> Result<Vec<tantivy::SegmentId>, CoreError> {
    Ok(index
        .searchable_segment_metas()
        .map_err(|err| {
            let context = format!("history text index: list {kind} index segments");
            if admitted {
                engine_error(&context, &err)
            } else {
                CoreError::Storage(format!("{context}: {err}"))
            }
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
    io: Option<&Arc<dyn HistoryTextFileIoPort>>,
    budget: &RequestBudgetV1,
) -> Result<(), CoreError> {
    let admitted = io.is_some();
    budget.checkpoint("history text: before compaction scan")?;
    let stale = segments_with_superseded_docs(index, kind, admitted)?;
    if stale.is_empty() {
        return Ok(());
    }
    budget.checkpoint("history text: before compacting superseded documents")?;
    let mut writer = open_writer(index, kind, writer_heap_bytes, admitted)?;
    let mut erased_worker_cause = false;
    let merged = writer.merge(&stale).wait().map_err(|err| {
        erased_worker_cause =
            matches!(err, TantivyError::ErrorInThread(_) | TantivyError::Poisoned);
        let context = format!("history text index: compact superseded {kind} documents");
        if admitted {
            engine_error(&context, &err)
        } else {
            CoreError::Storage(format!("{context}: {err}"))
        }
    });
    let work = merged
        .map(|_merged: Option<tantivy::SegmentMeta>| ())
        .and_then(|()| budget.checkpoint("history text: before compaction merge wait"));
    let drained = await_merges(writer, kind, admitted);
    let work = restore_worker_interruption(work, erased_worker_cause, io);
    let after = budget.checkpoint("history text: after compaction merge drain");
    let work = original_after_drain(work, after, "post-drain request budget");
    original_after_drain(work, drained, "history compaction merge drain")?;
    let remaining = segments_with_superseded_docs(index, kind, admitted)?;
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
    sync: Option<&crate::causal_profile::SyncTotalsV1>,
    budget: &RequestBudgetV1,
) -> Result<HistoryTextEpochReceiptV1, CoreError> {
    let staging = staging_dir(root, generation, epoch);
    let published = epoch_dir(root, generation, epoch);
    let build_sha256 = build_sha256(&build)?;
    if published.exists() {
        return replay_published(&published, epoch, build_sha256, None);
    }
    // A prior attempt's staging is retired even if this request was cancelled.
    if staging.exists() {
        std::fs::remove_dir_all(&staging).map_err(|err| {
            CoreError::Storage(format!(
                "history text index: remove stale staging directory {}: {err}",
                staging.display()
            ))
        })?;
    }
    budget.checkpoint("history text: before fresh raw epoch build")?;
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
    let mut by_kind = documents_by_kind(docs, budget)?;
    let mut written = 0_u64;
    for kind in [HistoryTextKindV1::Commit, HistoryTextKindV1::Diff] {
        budget.checkpoint("history text: before raw kind staging")?;
        let target = kind_dir(&staging, kind);
        std::fs::create_dir_all(&target).map_err(|err| {
            CoreError::Storage(format!(
                "history text index: create staging directory {}: {err}",
                target.display()
            ))
        })?;
        if let Some(base_dir) = &base_dir {
            inherit_kind_dir(&kind_dir(base_dir, kind), &target, budget)?;
        }
        let schema = KindSchema::build(kind);
        let docs = by_kind.remove(&kind).unwrap_or_default();
        written = written.saturating_add(write_kind(
            &schema,
            &target,
            docs,
            delete_first,
            writer_heap_bytes,
            None,
            budget,
        )?);
    }
    budget.checkpoint("history text: before raw manifest")?;
    HistoryTextManifest::describe(&staging, epoch, build_sha256, written)?
        .write_observed(&staging, sync)?;
    budget.checkpoint("history text: before raw epoch promotion")?;
    std::fs::rename(&staging, &published).map_err(|err| {
        CoreError::Storage(format!(
            "history text index: publish {} -> {}: {err}",
            staging.display(),
            published.display()
        ))
    })?;
    fsync_parent_observed(&published, sync)?;
    Ok(HistoryTextEpochReceiptV1 {
        docs_written: written,
    })
}

/// The qualified publication keeps a single pre-reserved scope for the entire
/// staging build, engine writes, manifest, and final directory promotion.
pub(super) fn recover_epoch_admitted(
    root: &Path,
    generation: &AuxiliaryGenerationKeyV1,
    epoch: AuxEpochV1,
    build: &HistoryTextBuildV1,
    io: &dyn HistoryTextFileIoPort,
) -> Result<Option<HistoryTextEpochReceiptV1>, CoreError> {
    io.validate_root(root)?;
    let staging = staging_dir(root, generation, epoch);
    let published = epoch_dir(root, generation, epoch);
    let expected = build_sha256(build)?;
    let recovered = io.recover_directory_move(&staging, &published)?;
    if recovered || io.metadata(&published)?.is_some() {
        replay_published(&published, epoch, expected, Some(io)).map(Some)
    } else {
        let _recovered_staging_delete = io.recover_tree_delete(&staging)?;
        if io.metadata(&staging)?.is_some() {
            let _removed = io.remove_tree(&staging)?;
        }
        Ok(None)
    }
}

/// Build a new epoch only after the retirement scope has checked recovery.
pub(super) fn publish_epoch_admitted(
    root: &Path,
    generation: &AuxiliaryGenerationKeyV1,
    epoch: AuxEpochV1,
    build: HistoryTextBuildV1,
    writer_heap_bytes: usize,
    io: &Arc<dyn HistoryTextFileIoPort>,
    budget: &RequestBudgetV1,
) -> Result<HistoryTextEpochReceiptV1, CoreError> {
    budget.checkpoint("history text: before admitted epoch build")?;
    io.validate_root(root)?;
    let staging = staging_dir(root, generation, epoch);
    let published = epoch_dir(root, generation, epoch);
    let build_sha256 = build_sha256(&build)?;
    // A concurrent published target is never replaced by this build.
    if io.metadata(&published)?.is_some() {
        return replay_published(&published, epoch, build_sha256, Some(io.as_ref()));
    }
    let (base_dir, docs, delete_first) = match build {
        HistoryTextBuildV1::Full { docs } => (None, docs, false),
        HistoryTextBuildV1::Incremental { base, upserts } => {
            if base >= epoch {
                return Err(CoreError::InvalidContract(format!(
                    "history text index: epoch {epoch} cannot build over epoch {base}"
                )));
            }
            (
                Some(require_servable_base_admitted(
                    root,
                    generation,
                    base,
                    io.as_ref(),
                )?),
                upserts,
                true,
            )
        }
    };
    let mut by_kind = documents_by_kind(docs, budget)?;
    let mut written = 0_u64;
    for kind in [HistoryTextKindV1::Commit, HistoryTextKindV1::Diff] {
        budget.checkpoint("history text: before admitted kind staging")?;
        let target = kind_dir(&staging, kind);
        io.ensure_directory(&target)?;
        if let Some(base_dir) = &base_dir {
            inherit_kind_dir_admitted(&kind_dir(base_dir, kind), &target, io.as_ref(), budget)?;
        }
        let schema = KindSchema::build(kind);
        let docs = by_kind.remove(&kind).unwrap_or_default();
        written = written.saturating_add(write_kind(
            &schema,
            &target,
            docs,
            delete_first,
            writer_heap_bytes,
            Some(io),
            budget,
        )?);
    }
    budget.checkpoint("history text: before admitted manifest")?;
    HistoryTextManifest::describe_admitted(&staging, epoch, build_sha256, written, io.as_ref())?
        .write_admitted(&staging, io.as_ref())?;
    budget.checkpoint("history text: before admitted epoch promotion")?;
    io.move_directory_if_absent(&staging, &published)?;
    sync_parent_admitted(&published, io.as_ref())?;
    Ok(HistoryTextEpochReceiptV1 {
        docs_written: written,
    })
}

#[cfg(test)]
mod drain_tests {
    use quanta_index_core::{CoreError, REQUEST_CANCELLED_CODE};

    use super::{original_after_drain, restore_observed_worker_interruption};

    #[test]
    fn worker_dead_error_uses_an_actual_interruption_witness_and_preserves_both_causes()
    -> Result<(), Box<dyn std::error::Error>> {
        let engine = CoreError::Storage("worker died before add_document".into());
        let observed = CoreError::Typed {
            code: REQUEST_CANCELLED_CODE,
            message: "actual admitted writer observed cancellation".into(),
        };
        match restore_observed_worker_interruption(engine, Ok(Some(observed))) {
            Err(CoreError::Typed { code, message })
                if code == REQUEST_CANCELLED_CODE
                    && message.contains("actual admitted writer observed cancellation")
                    && message.contains("worker died before add_document") =>
            {
                Ok(())
            }
            other => Err(format!("lost actual interruption witness: {other:?}").into()),
        }
    }

    #[test]
    fn missing_io_interruption_witness_does_not_relabel_an_engine_failure()
    -> Result<(), Box<dyn std::error::Error>> {
        let original = CoreError::Storage("worker failed independently of cancellation".into());
        let outcome = restore_observed_worker_interruption(original.clone(), Ok(None));
        if outcome != Err(original) {
            return Err(format!("fabricated an unobserved interruption: {outcome:?}").into());
        }
        Ok(())
    }

    #[test]
    fn original_cancellation_and_merge_retirement_failure_remain_distinguishable()
    -> Result<(), Box<dyn std::error::Error>> {
        let original = CoreError::Typed {
            code: REQUEST_CANCELLED_CODE,
            message: "cancelled after commit".into(),
        };
        let drain = CoreError::Storage("merge worker refused retirement".into());
        let result = original_after_drain(Err(original), Err(drain), "history writer merge drain");
        match result {
            Err(CoreError::Typed { code, message })
                if code == REQUEST_CANCELLED_CODE
                    && message.contains("cancelled after commit")
                    && message.contains("history writer merge drain")
                    && message.contains("merge worker refused retirement") =>
            {
                Ok(())
            }
            other => Err(
                format!("original typed cancellation or drain cause was lost: {other:?}").into(),
            ),
        }
    }
}
