//! A generation directory on disk: the delta base marker and inheriting a base generation.

#![expect(
    clippy::redundant_pub_crate,
    reason = "the module is private to the crate; `pub(crate)` is the visibility its items need across the crate's modules, and the workspace's `unreachable_pub = deny` forbids the bare `pub`"
)]

use crate::index_store::{lexical_sealed_identity_path, write_atomic_durable};
use crate::sealed_generation::{
    LEXICAL_QUARANTINE_RECEIPT_FILE_NAME, LEXICAL_SCRUB_RECEIPT_FILE_NAME,
    LEXICAL_SEALED_MANIFEST_FILE_NAME,
};
use crate::{
    LEXICAL_DELTA_BASE_FILE_NAME, LEXICAL_SEALED_IDENTITY_FILE_NAME, TANTIVY_INDEX_META_FILE_NAME,
    TANTIVY_LOCK_FILE_PREFIX, TANTIVY_MANAGED_FILE_NAME, sealed_generation,
};
use quanta_index_contract::ManifestGeneration;
use quanta_index_core::CoreError;
use std::fs::File;
use std::path::{Path, PathBuf};

pub(crate) fn lexical_delta_base_path(generation_dir: &Path) -> PathBuf {
    generation_dir.join(LEXICAL_DELTA_BASE_FILE_NAME)
}

/// The base generation this directory already carried forward, if any.
pub(crate) fn read_lexical_delta_base(
    generation_dir: &Path,
) -> Result<Option<ManifestGeneration>, CoreError> {
    let path = lexical_delta_base_path(generation_dir);
    let bytes = match std::fs::read(&path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => {
            return Err(CoreError::Storage(format!(
                "lexical: read delta base marker {}: {error}",
                path.display()
            )));
        }
    };
    let raw: u64 = ciborium::from_reader(bytes.as_slice()).map_err(|error| {
        CoreError::Storage(format!(
            "lexical: decode delta base marker {}: {error}",
            path.display()
        ))
    })?;
    Ok(Some(ManifestGeneration::new(raw)))
}

pub(crate) fn persist_lexical_delta_base(
    generation_dir: &Path,
    base_generation: ManifestGeneration,
) -> Result<(), CoreError> {
    let mut bytes = Vec::new();
    ciborium::into_writer(&base_generation.get(), &mut bytes).map_err(|error| {
        CoreError::Storage(format!(
            "lexical: encode delta base marker for generation {}: {error}",
            base_generation.get()
        ))
    })?;
    write_atomic_durable(
        &lexical_delta_base_path(generation_dir),
        &bytes,
        "delta base marker",
    )
}

/// Whether this directory already holds a materialized lexical index.
pub(crate) fn lexical_index_content_exists(generation_dir: &Path) -> bool {
    generation_dir.join(TANTIVY_INDEX_META_FILE_NAME).is_file()
}

/// Whether a generation-directory entry must be a private copy, not a link.
///
/// Tantivy rewrites these two under their existing names, so sharing the inode
/// would let one generation's commit mutate what another generation still
/// reads. Every other entry is either an immutable segment file or is replaced
/// by atomic rename (`write_atomic_durable`), both of which leave a hard link
/// pointing at the bytes it was created for.
pub(crate) fn is_generation_local_entry(file_name: &str) -> bool {
    matches!(
        file_name,
        TANTIVY_INDEX_META_FILE_NAME | TANTIVY_MANAGED_FILE_NAME
    )
}

/// Whether an entry belongs to a live writer and must not be inherited at all.
pub(crate) fn is_writer_lock_entry(file_name: &str) -> bool {
    file_name.starts_with(TANTIVY_LOCK_FILE_PREFIX)
}

/// Whether an entry is the base's seal (identity or content manifest) or
/// one of its scrub receipts.
///
/// A delta is unsealed until its own seal writes its own pair; inheriting
/// the base's would make a half-built delta claim the base's identity on
/// disk, which a crash before the seal would leave behind for the boot
/// scanner to refuse. The scrub receipts record a pass over, or a
/// corruption of, the base's committed bytes, which says nothing about
/// the delta.
pub(crate) fn is_seal_marker_entry(file_name: &str) -> bool {
    crate::sealed_generation::coverage::is_coverage_page(file_name)
        || matches!(
            file_name,
            LEXICAL_SEALED_IDENTITY_FILE_NAME
                | LEXICAL_SEALED_MANIFEST_FILE_NAME
                | LEXICAL_SCRUB_RECEIPT_FILE_NAME
                | LEXICAL_QUARANTINE_RECEIPT_FILE_NAME
                | crate::sealed_generation::coverage::SOURCE_FILE_COVERAGE_FILE_NAME
        )
}

/// Materializes one inherited entry: link the immutable ones, copy the rest.
pub(crate) fn inherit_generation_entry(source: &Path, target: &Path) -> Result<(), CoreError> {
    let file_name = source
        .file_name()
        .and_then(std::ffi::OsStr::to_str)
        .ok_or_else(|| {
            CoreError::Storage(format!(
                "lexical: base generation entry has no usable name: {}",
                source.display()
            ))
        })?;
    if is_generation_local_entry(file_name) {
        let _bytes_copied: u64 = std::fs::copy(source, target).map_err(|err| {
            CoreError::Storage(format!(
                "lexical: copy generation-local entry {} -> {}: {err}",
                source.display(),
                target.display()
            ))
        })?;
        return Ok(());
    }
    // Both paths live under one state root, so they are always on one device.
    // A failure here is a real storage fault, not a reason to quietly fall back
    // to a full byte copy and drop the incremental guarantee without saying so.
    std::fs::hard_link(source, target).map_err(|err| {
        CoreError::Storage(format!(
            "lexical: link inherited entry {} -> {}: {err}",
            source.display(),
            target.display()
        ))
    })
}

/// Refuse a delta base this build could not serve.
///
/// A delta inherits the base's index and text-authority shards byte for
/// byte, so a sealed base must have been built under the current text
/// normalizer and the current text-authority format; the typed manifest
/// refusals propagate. An unsealed base has no manifest yet and is
/// admitted — its own seal stamps the current versions.
pub(crate) fn ensure_base_generation_is_servable(base_dir: &Path) -> Result<(), CoreError> {
    if sealed_generation::manifest_path(base_dir).is_file() {
        let _manifest = sealed_generation::read_manifest(base_dir)?;
    }
    Ok(())
}

/// Materializes `src` into `dst` without replacing anything already present.
///
/// Delta semantics: an authority this generation published for itself outranks
/// the base's copy of the same authority, so an existing destination entry
/// wins. Only entries the target does not have are inherited.
///
/// Inherited entries are hard-linked rather than copied, so a delta's write
/// cost is proportional to what it changes instead of to the size of its base
/// (QI-BB-006). Tantivy segment files are immutable across commits and the
/// sidecars are replaced by atomic rename, so a shared inode is only ever read
/// through, never written through. The two entries Tantivy does rewrite in
/// place are copied instead — see [`is_generation_local_entry`]. Historical
/// gate rationale is recoverable with
/// `git show eff53181:docs/bugbash/sep-16/adr/G0-L-tantivy-snapshot-reuse.md`;
/// current qualification still requires a fresh source-bound receipt.
pub(crate) fn clone_generation_directory_preserving_existing(
    src: &Path,
    dst: &Path,
) -> Result<(), CoreError> {
    if !src.exists() {
        return Err(CoreError::NotReady(format!(
            "lexical: base generation missing at {}",
            src.display()
        )));
    }
    std::fs::create_dir_all(dst).map_err(|err| {
        CoreError::Storage(format!(
            "lexical: create cloned generation directory {}: {err}",
            dst.display()
        ))
    })?;
    for entry in std::fs::read_dir(src).map_err(|err| {
        CoreError::Storage(format!(
            "lexical: list base generation directory {}: {err}",
            src.display()
        ))
    })? {
        let entry = entry.map_err(|err| {
            CoreError::Storage(format!(
                "lexical: read base generation entry {}: {err}",
                src.display()
            ))
        })?;
        let entry_path = entry.path();
        let target_path = dst.join(entry.file_name());
        let file_type = entry.file_type().map_err(|err| {
            CoreError::Storage(format!(
                "lexical: inspect base generation entry {}: {err}",
                entry_path.display()
            ))
        })?;
        if file_type.is_dir() {
            clone_generation_directory_preserving_existing(&entry_path, &target_path)?;
            continue;
        }
        if target_path.exists() {
            continue;
        }
        if entry
            .file_name()
            .to_str()
            .is_some_and(|name| is_writer_lock_entry(name) || is_seal_marker_entry(name))
        {
            continue;
        }
        inherit_generation_entry(&entry_path, &target_path)?;
    }
    Ok(())
}

/// Refuse typed anything that would change a sealed generation.
pub(crate) fn ensure_unsealed(
    generation_dir: &Path,
    generation: ManifestGeneration,
    what: &str,
) -> Result<(), CoreError> {
    if lexical_sealed_identity_path(generation_dir).exists() {
        return Err(CoreError::Typed {
            code: quanta_index_contract::SearchPlaneErrorCodeV2::GenerationImmutable,
            message: format!(
                "lexical: generation {} is sealed; refusing to write {what} behind its sealed manifest",
                generation.get()
            ),
        });
    }
    Ok(())
}

/// Make the generation directory's entries durable before a door admits it.
pub(crate) fn sync_generation_directory(generation_dir: &Path) -> Result<(), CoreError> {
    File::open(generation_dir)
        .and_then(|directory| directory.sync_all())
        .map_err(|error| {
            CoreError::Storage(format!(
                "lexical: revalidate generation-directory durability {}: {error}",
                generation_dir.display()
            ))
        })
}

/// Sum of regular-file sizes under `root`, recursively.
///
/// What a reclaim or a quarantine discard reports giving back. A file
/// hard-linked into another generation counts here too; the disk frees it
/// when its last link goes. Writer lock files are transient and excluded.
pub(crate) fn generation_tree_bytes(root: &Path) -> Result<u64, CoreError> {
    let mut total = 0_u64;
    let mut pending = vec![root.to_path_buf()];
    while let Some(directory) = pending.pop() {
        let entries = std::fs::read_dir(&directory).map_err(|err| {
            CoreError::Storage(format!(
                "lexical: measure generation dir {}: {err}",
                directory.display()
            ))
        })?;
        for entry in entries {
            let entry = entry.map_err(|err| {
                CoreError::Storage(format!(
                    "lexical: measure generation entry in {}: {err}",
                    directory.display()
                ))
            })?;
            if is_writer_lock_entry(&entry.file_name().to_string_lossy()) {
                continue;
            }
            let metadata = entry.metadata().map_err(|err| {
                CoreError::Storage(format!(
                    "lexical: measure generation entry {}: {err}",
                    entry.path().display()
                ))
            })?;
            if metadata.is_dir() {
                pending.push(entry.path());
            } else if metadata.is_file() {
                total = total.saturating_add(metadata.len());
            }
        }
    }
    Ok(total)
}
