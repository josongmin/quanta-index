//! Opening a generation's index and persisting its sealed identity durably.

#![expect(
    clippy::redundant_pub_crate,
    reason = "the module is private to the crate; `pub(crate)` is the visibility its items need across the crate's modules, and the workspace's `unreachable_pub = deny` forbids the bare `pub`"
)]

use crate::analyzer::register_analyzers;
use crate::normalize::{TEXT_NORMALIZER_VERSION, TextNormalizerVersion};
#[cfg(test)]
use crate::publication_faults::{Cut, Side, authority_cut_core, root_cut_core};
use crate::{
    DURABLE_WRITE_TEMPORARY_MARKER, LEXICAL_SEALED_IDENTITY_FILE_NAME, SchemaFields,
    TANTIVY_INDEX_META_FILE_NAME,
};
use quanta_index_contract::GenerationSnapshot;
use quanta_index_core::CoreError;
use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use tantivy::Index;

const MAX_SEALED_IDENTITY_BYTES: usize = 4096;
/// Format of a writable index whose merge engine records exact live BM25 totals.
///
/// This marker is written before creating the first index commit, and the
/// sealed manifest takes over as the serving authority after publication.
const LEXICAL_UNSEALED_INDEX_FORMAT_FILE_NAME: &str = "search-corpus-index-format.cbor";
const LEXICAL_UNSEALED_INDEX_FORMAT_VERSION: u32 = 2;
const MAX_UNSEALED_INDEX_FORMAT_BYTES: usize = 9;

fn unsealed_index_format_refusal(generation_dir: &Path, detail: &str) -> CoreError {
    CoreError::Typed {
        code: quanta_index_contract::SearchPlaneErrorCodeV2::GenerationManifestFormatUnsupported,
        message: format!(
            "lexical: unsealed index {} cannot prove exact live BM25 token totals ({detail}); discard the incomplete generation and rebuild it",
            generation_dir.display()
        ),
    }
}

fn unsealed_index_format_path(generation_dir: &Path) -> PathBuf {
    generation_dir.join(LEXICAL_UNSEALED_INDEX_FORMAT_FILE_NAME)
}

fn read_unsealed_index_format(generation_dir: &Path) -> Result<Option<u32>, CoreError> {
    let marker = Path::new(LEXICAL_UNSEALED_INDEX_FORMAT_FILE_NAME);
    let mut file = match crate::sealed_generation::open_regular_nofollow(generation_dir, marker) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => {
            return Err(unsealed_index_format_refusal(
                generation_dir,
                &format!("cannot open format marker: {error}"),
            ));
        }
    };
    let bytes =
        crate::sealed_generation::read_opened_bounded(&mut file, MAX_UNSEALED_INDEX_FORMAT_BYTES)
            .map_err(|error| {
            unsealed_index_format_refusal(
                generation_dir,
                &format!("cannot read format marker: {error}"),
            )
        })?;
    let version: u32 = crate::channel_payloads::decode_cbor_exact(&bytes).map_err(|error| {
        unsealed_index_format_refusal(generation_dir, &format!("invalid format marker: {error}"))
    })?;
    Ok(Some(version))
}

fn index_meta_entry_present(generation_dir: &Path) -> Result<bool, CoreError> {
    match std::fs::symlink_metadata(generation_dir.join(TANTIVY_INDEX_META_FILE_NAME)) {
        Ok(_metadata) => Ok(true),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(CoreError::Storage(format!(
            "lexical: inspect unsealed index commit at {}: {error}",
            generation_dir.display()
        ))),
    }
}

/// An existing unsealed index may be resumed only when this producer wrote it.
/// A pre-upgrade no-delete segment can still carry an approximate BM25 header.
pub(crate) fn require_current_unsealed_index_format_if_materialized(
    generation_dir: &Path,
) -> Result<(), CoreError> {
    if !index_meta_entry_present(generation_dir)? {
        return Ok(());
    }
    match read_unsealed_index_format(generation_dir)? {
        Some(LEXICAL_UNSEALED_INDEX_FORMAT_VERSION) => Ok(()),
        Some(version) => Err(unsealed_index_format_refusal(
            generation_dir,
            &format!("format {version}, expected {LEXICAL_UNSEALED_INDEX_FORMAT_VERSION}"),
        )),
        None => Err(unsealed_index_format_refusal(
            generation_dir,
            "format marker is missing",
        )),
    }
}

/// Establish current producer provenance durably before an index is created
/// or a proved current-format base is copied into an empty delta target.
pub(crate) fn ensure_current_unsealed_index_format_for_writer(
    generation_dir: &Path,
) -> Result<(), CoreError> {
    require_current_unsealed_index_format_if_materialized(generation_dir)?;
    match read_unsealed_index_format(generation_dir)? {
        Some(LEXICAL_UNSEALED_INDEX_FORMAT_VERSION) => Ok(()),
        Some(version) => Err(unsealed_index_format_refusal(
            generation_dir,
            &format!("format {version}, expected {LEXICAL_UNSEALED_INDEX_FORMAT_VERSION}"),
        )),
        None => {
            let mut bytes = Vec::new();
            ciborium::into_writer(&LEXICAL_UNSEALED_INDEX_FORMAT_VERSION, &mut bytes).map_err(
                |error| {
                    CoreError::Storage(format!("lexical: encode unsealed index format: {error}"))
                },
            )?;
            std::fs::create_dir_all(generation_dir).map_err(|error| {
                CoreError::Storage(format!(
                    "lexical: create generation directory {} for index format: {error}",
                    generation_dir.display()
                ))
            })?;
            write_atomic_durable(
                &unsealed_index_format_path(generation_dir),
                &bytes,
                "unsealed index format",
            )
        }
    }
}
static TEMP_SEQUENCE: AtomicU64 = AtomicU64::new(0);

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
    let root = crate::sealed_generation::open_generation_dir_nofollow(generation_dir).map_err(
        |error| {
            CoreError::Storage(format!(
                "lexical: open sealed generation directory {}: {error}",
                generation_dir.display()
            ))
        },
    )?;
    let directory = crate::sealed_generation::SealedIndexDirectory::from_opened(root);
    finish_open_sealed_index(generation_dir, directory)
}

pub(crate) fn open_sealed_index_at(
    generation_dir: &Path,
    root: &File,
    sealed_meta: Vec<u8>,
) -> Result<Index, CoreError> {
    let directory = crate::sealed_generation::SealedIndexDirectory::from_opened_with_meta(
        root.try_clone().map_err(|error| {
            CoreError::Storage(format!(
                "lexical: clone sealed generation directory {}: {error}",
                generation_dir.display()
            ))
        })?,
        sealed_meta,
    );
    finish_open_sealed_index(generation_dir, directory)
}

fn finish_open_sealed_index(
    generation_dir: &Path,
    directory: crate::sealed_generation::SealedIndexDirectory,
) -> Result<Index, CoreError> {
    let index = Index::open(directory).map_err(|error| {
        CoreError::Storage(format!(
            "lexical: strict open existing generation {}: {error}",
            generation_dir.display()
        ))
    })?;
    if index.schema() != SchemaFields::build().schema {
        return Err(CoreError::Typed {
            code:
                quanta_index_contract::SearchPlaneErrorCodeV2::GenerationManifestFormatUnsupported,
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
    match std::fs::symlink_metadata(path.join(crate::LEXICAL_DELTA_CLONE_INTENT_FILE_NAME)) {
        Ok(_) => {
            return Err(CoreError::Typed {
                code: quanta_index_contract::SearchPlaneErrorCodeV2::DeltaBaseUnresolved,
                message: "lexical: finish the pending delta clone before opening a writer".into(),
            });
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => {
            return Err(CoreError::Storage(format!(
                "lexical: inspect delta clone intent: {error}"
            )));
        }
    }
    std::fs::create_dir_all(path).map_err(|err| {
        CoreError::Storage(format!(
            "lexical: create generation directory {}: {err}",
            path.display()
        ))
    })?;
    ensure_current_unsealed_index_format_for_writer(path)?;
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
    write_atomic_durable_with(path, label, |file| file.write_all(bytes))
}

/// Replacing an interrupted metadata copy never writes through a base inode
/// or destination symlink. Copy through a fresh private file before rename.
pub(crate) fn copy_atomic_durable(source: &Path, target: &Path) -> Result<(), CoreError> {
    let parent = source
        .parent()
        .ok_or_else(|| CoreError::Storage("clone source has no parent".into()))?;
    let name = source
        .file_name()
        .ok_or_else(|| CoreError::Storage("clone source has no name".into()))?;
    let mut source = crate::sealed_generation::open_regular_nofollow(parent, Path::new(name))
        .map_err(|error| CoreError::Storage(format!("lexical: open cloned metadata: {error}")))?;
    write_atomic_durable_with(target, "cloned metadata", |target| {
        let expected_bytes = source.metadata()?.len();
        let copied = std::io::copy(&mut source, target)?;
        if copied != expected_bytes || source.metadata()?.len() != expected_bytes {
            return Err(std::io::Error::other("clone source length changed"));
        }
        Ok(())
    })
}

fn write_atomic_durable_with(
    path: &Path,
    label: &str,
    write: impl FnOnce(&mut File) -> std::io::Result<()>,
) -> Result<(), CoreError> {
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
    #[cfg(test)]
    root_cut_core(Cut::RootWrite, Side::Before, path)?;
    write(&mut file).map_err(|error| {
        CoreError::Storage(format!(
            "lexical: write {label} temporary {}: {error}",
            temporary.display()
        ))
    })?;
    #[cfg(test)]
    root_cut_core(Cut::RootWrite, Side::After, path)?;
    #[cfg(test)]
    root_cut_core(Cut::RootFileSync, Side::Before, path)?;
    crate::causal_profile::timed_sync("atomic_file", || file.sync_all()).map_err(|error| {
        CoreError::Storage(format!(
            "lexical: fsync {label} temporary {}: {error}",
            temporary.display()
        ))
    })?;
    #[cfg(test)]
    root_cut_core(Cut::RootFileSync, Side::After, path)?;
    drop(file);
    #[cfg(test)]
    root_cut_core(Cut::RootRename, Side::Before, path)?;
    std::fs::rename(&temporary, path).map_err(|error| {
        CoreError::Storage(format!(
            "lexical: rename {label} temporary {} to {}: {error}",
            temporary.display(),
            path.display()
        ))
    })?;
    #[cfg(test)]
    root_cut_core(Cut::RootRename, Side::After, path)?;
    #[cfg(test)]
    root_cut_core(Cut::RootDirectorySync, Side::Before, path)?;
    File::open(parent)
        .and_then(|directory| {
            crate::causal_profile::timed_sync("atomic_parent", || directory.sync_all())
        })
        .map_err(|error| {
            CoreError::Storage(format!(
                "lexical: fsync {label} parent {}: {error}",
                parent.display()
            ))
        })?;
    #[cfg(test)]
    root_cut_core(Cut::RootDirectorySync, Side::After, path)?;
    Ok(())
}

/// Remove the temporary files an interrupted durable write left behind.
///
/// `write_atomic_durable` names its temporaries `.<file>.tmp-<pid>-<n>`
/// and renames them into place only once fsynced, so any such file at seal
/// time is a crash's leftover with no owner. The caller must own the
/// directory's publication boundary; this must never run against active writers.
pub(crate) fn remove_publish_leftovers(
    directory: &Path,
    sync_domain: &'static str,
) -> Result<(), CoreError> {
    let mut removed_any = false;
    for entry in std::fs::read_dir(directory).map_err(|error| {
        CoreError::Storage(format!(
            "lexical: list {} before sealing: {error}",
            directory.display()
        ))
    })? {
        let entry = entry.map_err(|error| {
            CoreError::Storage(format!(
                "lexical: read entry of {} before sealing: {error}",
                directory.display()
            ))
        })?;
        let name = entry.file_name();
        let Some(name) = name.to_str() else {
            continue;
        };
        if !is_durable_write_temporary(name) {
            continue;
        }
        #[cfg(test)]
        authority_cut_core(Cut::RootTemporaryCleanup, Side::Before, directory)?;
        std::fs::remove_file(entry.path()).map_err(|error| {
            CoreError::Storage(format!(
                "lexical: remove interrupted publish leftover {}: {error}",
                entry.path().display()
            ))
        })?;
        #[cfg(test)]
        authority_cut_core(Cut::RootTemporaryCleanup, Side::After, directory)?;
        removed_any = true;
    }
    if removed_any {
        #[cfg(test)]
        authority_cut_core(Cut::RootTemporaryDirectorySync, Side::Before, directory)?;
        File::open(directory)
            .and_then(|directory| {
                crate::causal_profile::timed_sync(sync_domain, || directory.sync_all())
            })
            .map_err(|error| {
                CoreError::Storage(format!(
                    "lexical: fsync {} after removing publish leftovers: {error}",
                    directory.display()
                ))
            })?;
        #[cfg(test)]
        authority_cut_core(Cut::RootTemporaryDirectorySync, Side::After, directory)?;
    }
    Ok(())
}

/// Publish a receipt inside the generation whose directory descriptor was
/// authenticated. A later pathname replacement cannot redirect the receipt
/// into another generation.
pub(crate) fn write_atomic_durable_at(
    root: &File,
    generation_dir: &Path,
    name: &str,
    bytes: &[u8],
    label: &str,
) -> Result<(), CoreError> {
    use rustix::fs::{Mode, OFlags, openat, renameat};

    let mut components = Path::new(name).components();
    if name.contains('/')
        || !matches!(components.next(), Some(Component::Normal(_)))
        || components.next().is_some()
    {
        return Err(CoreError::InvalidContract(format!(
            "lexical: {label} name is not a top-level generation entry"
        )));
    }

    let temporary = format!(
        ".{name}{DURABLE_WRITE_TEMPORARY_MARKER}{}-{}",
        std::process::id(),
        TEMP_SEQUENCE.fetch_add(1, Ordering::Relaxed)
    );
    let mut file = File::from(
        openat(
            root,
            temporary.as_str(),
            OFlags::WRONLY | OFlags::CREATE | OFlags::EXCL | OFlags::CLOEXEC | OFlags::NOFOLLOW,
            Mode::RUSR | Mode::WUSR,
        )
        .map_err(|error| {
            CoreError::Storage(format!(
                "lexical: create {label} temporary in {}: {error}",
                generation_dir.display()
            ))
        })?,
    );
    file.write_all(bytes).map_err(|error| {
        CoreError::Storage(format!(
            "lexical: write {label} temporary in {}: {error}",
            generation_dir.display()
        ))
    })?;
    crate::causal_profile::timed_sync("atomic_at_file", || file.sync_all()).map_err(|error| {
        CoreError::Storage(format!(
            "lexical: fsync {label} temporary in {}: {error}",
            generation_dir.display()
        ))
    })?;
    drop(file);
    renameat(root, temporary.as_str(), root, name).map_err(|error| {
        CoreError::Storage(format!(
            "lexical: publish {label} in {}: {error}",
            generation_dir.display()
        ))
    })?;
    crate::causal_profile::timed_sync("atomic_at_parent", || root.sync_all()).map_err(|error| {
        CoreError::Storage(format!(
            "lexical: fsync {label} generation {}: {error}",
            generation_dir.display()
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
        code: quanta_index_contract::SearchPlaneErrorCodeV2::GenerationNormalizerUnsupported,
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
        code: quanta_index_contract::SearchPlaneErrorCodeV2::GenerationSidecarCorrupt,
        message: format!(
            "lexical: generation {} does not match its manifest: {name}: {reason}",
            generation_dir.display()
        ),
    }
}

pub(crate) fn read_lexical_sealed_identity(
    generation_dir: &Path,
) -> Result<GenerationSnapshot, CoreError> {
    let root = crate::sealed_generation::open_generation_dir_nofollow(generation_dir).map_err(
        |error| {
            CoreError::Storage(format!(
                "lexical: open sealed generation directory {}: {error}",
                generation_dir.display()
            ))
        },
    )?;
    read_lexical_sealed_identity_at(generation_dir, &root)
}

/// A sealed identity entry is absent only when no directory entry exists.
/// A dangling symlink or special file still fences an incomplete discard.
pub(crate) fn sealed_identity_entry_present(generation_dir: &Path) -> Result<bool, CoreError> {
    let path = lexical_sealed_identity_path(generation_dir);
    crate::sealed_generation::optional_entry_metadata(&path)
        .map(|metadata| metadata.is_some())
        .map_err(|error| {
            CoreError::Storage(format!(
                "lexical: inspect sealed identity {}: {error}",
                path.display()
            ))
        })
}

pub(crate) fn read_lexical_sealed_identity_at(
    generation_dir: &Path,
    root: &File,
) -> Result<GenerationSnapshot, CoreError> {
    let path = lexical_sealed_identity_path(generation_dir);
    let present = crate::sealed_generation::optional_entry_at(
        root,
        Path::new(LEXICAL_SEALED_IDENTITY_FILE_NAME),
    )
    .map_err(|error| {
        CoreError::Storage(format!(
            "lexical: inspect sealed identity {}: {error}",
            path.display()
        ))
    })?;
    if !present {
        return Err(CoreError::Typed {
            code: quanta_index_contract::SearchPlaneErrorCodeV2::GenerationIdentityIncomplete,
            message: format!(
                "lexical: incomplete generation has no sealed identity at {}",
                path.display()
            ),
        });
    }
    let mut file = crate::sealed_generation::open_regular_below(
        root,
        Path::new(LEXICAL_SEALED_IDENTITY_FILE_NAME),
    )
    .map_err(|error| {
        if crate::sealed_generation::is_unsafe_artifact_path(&error) {
            CoreError::Typed {
                code: quanta_index_contract::SearchPlaneErrorCodeV2::GenerationSidecarCorrupt,
                message: format!(
                    "lexical: sealed identity {} is not a regular file within its generation",
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
    // Repo/revision IDs are at most 512 bytes each and the seal writes a
    // SHA-256 digest. A larger sidecar is not a valid identity; cap the read
    // so a damaged file cannot make the maintenance liveness probe allocate
    // without bound.
    let on_disk = file
        .metadata()
        .map_err(|error| {
            CoreError::Storage(format!(
                "lexical: inspect sealed generation identity {}: {error}",
                path.display()
            ))
        })?
        .len();
    if !matches!(usize::try_from(on_disk), Ok(bytes) if bytes <= MAX_SEALED_IDENTITY_BYTES) {
        return Err(sidecar_corrupt(
            generation_dir,
            LEXICAL_SEALED_IDENTITY_FILE_NAME,
            &format!("exceeds {MAX_SEALED_IDENTITY_BYTES} bytes"),
        ));
    }
    let bytes = crate::sealed_generation::read_opened_bounded(&mut file, MAX_SEALED_IDENTITY_BYTES)
        .map_err(|error| {
            if matches!(
                error.kind(),
                std::io::ErrorKind::InvalidData | std::io::ErrorKind::UnexpectedEof
            ) {
                sidecar_corrupt(
                    generation_dir,
                    LEXICAL_SEALED_IDENTITY_FILE_NAME,
                    &format!("cannot read admitted bytes: {error}"),
                )
            } else {
                CoreError::Storage(format!(
                    "lexical: read sealed generation identity {}: {error}",
                    path.display()
                ))
            }
        })?;
    crate::channel_payloads::decode_cbor_exact(bytes.as_slice()).map_err(|error| {
        sidecar_corrupt(
            generation_dir,
            LEXICAL_SEALED_IDENTITY_FILE_NAME,
            &format!("cannot decode: {error}"),
        )
    })
}

pub(crate) fn validate_lexical_sealed_identity(
    observed: &GenerationSnapshot,
    candidate: &GenerationSnapshot,
) -> Result<(), CoreError> {
    if observed != candidate {
        return Err(CoreError::Typed {
            code: quanta_index_contract::SearchPlaneErrorCodeV2::GenerationIdentityDigestMismatch,
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

#[cfg(test)]
mod sealed_identity_probe_tests {
    use super::{
        lexical_sealed_identity_path, read_lexical_sealed_identity, read_lexical_sealed_identity_at,
    };
    use quanta_index_contract::{
        GenerationSnapshot, ManifestGeneration, RepoId, RevisionId, SearchPlaneTrackKind,
    };
    use quanta_index_core::CoreError;

    #[test]
    fn oversized_sealed_identity_is_refused_before_decode() {
        let temp = tempfile::tempdir().expect("fixture generation directory");
        let generation = temp.path().join("family/g1");
        std::fs::create_dir_all(&generation).expect("generation directory");
        std::fs::write(lexical_sealed_identity_path(&generation), vec![b'x'; 4097])
            .expect("oversized identity fixture");
        let error = read_lexical_sealed_identity(&generation)
            .expect_err("oversized sidecar must be refused");
        assert!(
            matches!(error, CoreError::Typed { code: quanta_index_contract::SearchPlaneErrorCodeV2::GenerationSidecarCorrupt, message } if message.contains("exceeds 4096 bytes"))
        );
    }

    #[test]
    fn sealed_identity_rejects_trailing_cbor() -> Result<(), Box<dyn std::error::Error>> {
        let temp = tempfile::tempdir()?;
        let generation = temp.path().join("family/g1");
        std::fs::create_dir_all(&generation)?;
        let identity = GenerationSnapshot {
            repo_id: RepoId::new("repo")?,
            revision_id: RevisionId::new("revision")?,
            track: SearchPlaneTrackKind::Lexical,
            manifest_generation: ManifestGeneration::new(1),
            manifest_digest: "digest".into(),
        };
        let mut bytes = crate::channel_payloads::encode_cbor(&identity, "identity test")?;
        bytes.push(0xff);
        std::fs::write(lexical_sealed_identity_path(&generation), bytes)?;
        match read_lexical_sealed_identity(&generation) {
            Err(CoreError::Typed {
                code: quanta_index_contract::SearchPlaneErrorCodeV2::GenerationSidecarCorrupt,
                message,
            }) if message.contains("trailing CBOR bytes") => Ok(()),
            other => Err(format!("trailing identity bytes were admitted: {other:?}").into()),
        }
    }

    #[test]
    fn opened_identity_does_not_follow_a_replaced_generation()
    -> Result<(), Box<dyn std::error::Error>> {
        let parent = tempfile::tempdir()?;
        let generation = parent.path().join("generation");
        std::fs::create_dir(&generation)?;
        let original = GenerationSnapshot {
            repo_id: RepoId::new("repo")?,
            revision_id: RevisionId::new("revision")?,
            track: SearchPlaneTrackKind::Lexical,
            manifest_generation: ManifestGeneration::new(1),
            manifest_digest: "shared-digest".into(),
        };
        std::fs::write(
            lexical_sealed_identity_path(&generation),
            crate::channel_payloads::encode_cbor(&original, "identity test")?,
        )?;
        let opened = crate::sealed_generation::open_generation_dir_nofollow(&generation)?;
        std::fs::rename(&generation, parent.path().join("old-generation"))?;
        std::fs::create_dir(&generation)?;
        let mut replacement = original.clone();
        replacement.manifest_generation = ManifestGeneration::new(2);
        std::fs::write(
            lexical_sealed_identity_path(&generation),
            crate::channel_payloads::encode_cbor(&replacement, "identity test")?,
        )?;
        if read_lexical_sealed_identity_at(&generation, &opened)? != original
            || read_lexical_sealed_identity(&generation)? != replacement
        {
            return Err("identity reader followed a replacement generation".into());
        }
        Ok(())
    }

    #[cfg(unix)]
    #[test]
    fn fifo_and_symlink_identity_are_refused_without_opening_their_targets() {
        use std::sync::mpsc;
        use std::time::Duration;

        let temp = tempfile::tempdir().expect("fixture generation directory");
        let generation = temp.path().join("family/g1");
        std::fs::create_dir_all(&generation).expect("generation directory");
        let identity = lexical_sealed_identity_path(&generation);
        assert!(
            std::process::Command::new("mkfifo")
                .arg(&identity)
                .status()
                .expect("create FIFO identity")
                .success()
        );
        let (sender, receiver) = mpsc::channel();
        let reader_generation = generation.clone();
        let reader = std::thread::spawn(move || {
            let refused = matches!(
                read_lexical_sealed_identity(&reader_generation),
                Err(CoreError::Typed {
                    code: quanta_index_contract::SearchPlaneErrorCodeV2::GenerationSidecarCorrupt,
                    ..
                })
            );
            sender.send(refused).expect("report identity refusal");
        });
        let fifo_result = receiver.recv_timeout(Duration::from_secs(5));
        if matches!(fifo_result, Err(mpsc::RecvTimeoutError::Timeout)) {
            // A broken blocking reader must be released so the test process
            // reports a bounded failure instead of hanging the whole suite.
            drop(
                std::fs::OpenOptions::new()
                    .write(true)
                    .open(&identity)
                    .expect("unblock FIFO reader"),
            );
        }
        reader.join().expect("identity reader thread");
        assert!(matches!(fifo_result, Ok(true)));
        std::fs::remove_file(&identity).expect("remove FIFO");
        let outside = tempfile::NamedTempFile::new().expect("outside identity");
        std::os::unix::fs::symlink(outside.path(), &identity).expect("create identity symlink");
        assert!(matches!(
            read_lexical_sealed_identity(&generation),
            Err(CoreError::Typed {
                code: quanta_index_contract::SearchPlaneErrorCodeV2::GenerationSidecarCorrupt,
                ..
            })
        ));
    }
}
