//! Opening a generation's index and persisting its sealed identity durably.

#![expect(
    clippy::redundant_pub_crate,
    reason = "the module is private to the crate; `pub(crate)` is the visibility its items need across the crate's modules, and the workspace's `unreachable_pub = deny` forbids the bare `pub`"
)]

use crate::analyzer::register_analyzers;
use crate::normalize::{TEXT_NORMALIZER_VERSION, TextNormalizerVersion};
use crate::{
    DURABLE_WRITE_TEMPORARY_MARKER, LEXICAL_SEALED_IDENTITY_FILE_NAME, SchemaFields,
    sealed_generation,
};
use quanta_index_contract::GenerationSnapshot;
use quanta_index_core::{CoreError, GENERATION_SIDECAR_CORRUPT_CODE};
use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use tantivy::Index;

/// Open an existing generation's index strictly, never creating or
/// repairing one, with the tokenizers registered.
///
/// A cached writer handle could survive deletion or corruption of the
/// backing files, so every door bypasses that cache and opens the durable
/// directory. The committed schema must be the one this build writes: an
/// index written under another — a field no longer indexed, a column this
/// build restricts through missing — is refused typed, never served with
/// queries it cannot answer.
pub(crate) fn open_sealed_index(generation_dir: &Path) -> Result<Index, CoreError> {
    let index = Index::open_in_dir(generation_dir).map_err(|error| {
        CoreError::Storage(format!(
            "lexical: strict open existing generation {}: {error}",
            generation_dir.display()
        ))
    })?;
    if index.schema() != SchemaFields::build().schema {
        return Err(CoreError::Typed {
            code: sealed_generation::GENERATION_MANIFEST_FORMAT_UNSUPPORTED_CODE.to_string(),
            message: format!(
                "lexical: generation {} was indexed under a schema this build does not write; it must be rebuilt",
                generation_dir.display()
            ),
        });
    }
    register_analyzers(&index);
    Ok(index)
}

/// Whether `name` is a durable write's temporary file
/// (`.<file>.tmp-<pid>-<n>`), which only a crash leaves behind.
pub(crate) fn is_durable_write_temporary(name: &str) -> bool {
    name.starts_with('.') && name.contains(DURABLE_WRITE_TEMPORARY_MARKER)
}

/// Open or create the Tantivy index at `path` under the adapter's schema.
///
/// Free function rather than a method so [`WriterCache`] can call it without
/// holding a reference to the adapter (which would require re-entering the
/// cache mutex).
pub(crate) fn open_or_create_index(fields: &SchemaFields, path: &Path) -> Result<Index, CoreError> {
    std::fs::create_dir_all(path).map_err(|err| {
        CoreError::Storage(format!(
            "lexical: create generation directory {}: {err}",
            path.display()
        ))
    })?;
    let directory = tantivy::directory::MmapDirectory::open(path).map_err(|err| {
        CoreError::Storage(format!(
            "lexical: open generation directory {}: {err}",
            path.display()
        ))
    })?;
    let index = Index::builder()
        .schema(fields.schema.clone())
        .open_or_create(directory)
        .map_err(|err| CoreError::Storage(format!("lexical: open generation index: {err}")))?;
    register_analyzers(&index);
    Ok(index)
}

pub(crate) fn lexical_sealed_identity_path(generation_dir: &Path) -> PathBuf {
    generation_dir.join(LEXICAL_SEALED_IDENTITY_FILE_NAME)
}

/// Write `bytes` to `path` durably.
///
/// A uniquely named temporary beside it, fsync, rename over `path`, fsync
/// the parent. A crash leaves either the old file or the new one, never a
/// torn one, plus at most a temporary the seal removes
/// ([`is_durable_write_temporary`]).
pub(crate) fn write_atomic_durable(
    path: &Path,
    bytes: &[u8],
    label: &str,
) -> Result<(), CoreError> {
    static TEMP_SEQUENCE: AtomicU64 = AtomicU64::new(0);
    let parent = path.parent().ok_or_else(|| {
        CoreError::Storage(format!(
            "lexical: {label} path has no parent: {}",
            path.display()
        ))
    })?;
    let file_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| {
            CoreError::Storage(format!(
                "lexical: {label} has no UTF-8 file name: {}",
                path.display()
            ))
        })?;
    let sequence = TEMP_SEQUENCE.fetch_add(1, Ordering::Relaxed);
    let temporary = parent.join(format!(
        ".{file_name}{DURABLE_WRITE_TEMPORARY_MARKER}{}-{sequence}",
        std::process::id()
    ));
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&temporary)
        .map_err(|error| {
            CoreError::Storage(format!(
                "lexical: create {label} temporary {}: {error}",
                temporary.display()
            ))
        })?;
    file.write_all(bytes).map_err(|error| {
        CoreError::Storage(format!(
            "lexical: write {label} temporary {}: {error}",
            temporary.display()
        ))
    })?;
    file.sync_all().map_err(|error| {
        CoreError::Storage(format!(
            "lexical: fsync {label} temporary {}: {error}",
            temporary.display()
        ))
    })?;
    drop(file);
    std::fs::rename(&temporary, path).map_err(|error| {
        CoreError::Storage(format!(
            "lexical: rename {label} temporary {} to {}: {error}",
            temporary.display(),
            path.display()
        ))
    })?;
    File::open(parent)
        .and_then(|directory| directory.sync_all())
        .map_err(|error| {
            CoreError::Storage(format!(
                "lexical: fsync {label} parent {}: {error}",
                parent.display()
            ))
        })
}

pub(crate) fn persist_lexical_sealed_identity(
    generation_dir: &Path,
    identity: &GenerationSnapshot,
) -> Result<(), CoreError> {
    let mut bytes = Vec::new();
    ciborium::into_writer(identity, &mut bytes).map_err(|error| {
        CoreError::Storage(format!(
            "lexical: encode sealed generation identity for repo={} revision={} generation={}: {error}",
            identity.repo_id.as_str(),
            identity.revision_id.as_str(),
            identity.manifest_generation.get(),
        ))
    })?;
    write_atomic_durable(
        &lexical_sealed_identity_path(generation_dir),
        &bytes,
        "sealed generation identity",
    )
}

pub(crate) fn normalizer_unsupported(path: &Path, built_with: TextNormalizerVersion) -> CoreError {
    CoreError::Typed {
        code: "GENERATION_NORMALIZER_UNSUPPORTED".to_string(),
        message: format!(
            "lexical: sealed generation {} was built under text normalizer {built_with} (this build runs {TEXT_NORMALIZER_VERSION}); it must be rebuilt, never served with mismatched text semantics",
            path.display()
        ),
    }
}

/// The typed refusal for a sealed generation that is not what its
/// manifest committed to.
pub(crate) fn sidecar_corrupt(generation_dir: &Path, name: &str, reason: &str) -> CoreError {
    CoreError::Typed {
        code: GENERATION_SIDECAR_CORRUPT_CODE.to_string(),
        message: format!(
            "lexical: generation {} does not match its manifest: {name}: {reason}",
            generation_dir.display()
        ),
    }
}

pub(crate) fn read_lexical_sealed_identity(
    generation_dir: &Path,
) -> Result<GenerationSnapshot, CoreError> {
    let path = lexical_sealed_identity_path(generation_dir);
    let bytes = std::fs::read(&path).map_err(|error| {
        if error.kind() == std::io::ErrorKind::NotFound {
            CoreError::Typed {
                code: "GENERATION_IDENTITY_INCOMPLETE".to_string(),
                message: format!(
                    "lexical: incomplete generation has no sealed identity at {}",
                    path.display()
                ),
            }
        } else {
            CoreError::Storage(format!(
                "lexical: read sealed generation identity {}: {error}",
                path.display()
            ))
        }
    })?;
    ciborium::from_reader(bytes.as_slice()).map_err(|error| {
        CoreError::Storage(format!(
            "lexical: decode sealed generation identity {}: {error}",
            path.display()
        ))
    })
}

pub(crate) fn validate_lexical_sealed_identity(
    observed: &GenerationSnapshot,
    candidate: &GenerationSnapshot,
) -> Result<(), CoreError> {
    if observed != candidate {
        return Err(CoreError::Typed {
            code: "GENERATION_IDENTITY_DIGEST_MISMATCH".to_string(),
            message: format!(
                "lexical: durable generation identity mismatch for repo={} revision={} generation={}",
                candidate.repo_id.as_str(),
                candidate.revision_id.as_str(),
                candidate.manifest_generation.get(),
            ),
        });
    }
    Ok(())
}
