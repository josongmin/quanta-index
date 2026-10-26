// Licensed to the Apache Software Foundation (ASF) under one
// or more contributor license agreements.  See the NOTICE file
// distributed with this work for additional information
// regarding copyright ownership.  The ASF licenses this file
// to you under the Apache License, Version 2.0 (the
// "License"); you may not use this file except in compliance
// with the License.  You may obtain a copy of the License at
//
//   http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing,
// software distributed under the License is distributed on an
// "AS IS" BASIS, WITHOUT WARRANTIES OR CONDITIONS OF ANY
// KIND, either express or implied.  See the License for the
// specific language governing permissions and limitations
// under the License.

//! An object store implementation for a local filesystem
pub mod admission;

use std::fs::{metadata, symlink_metadata, File, Metadata, OpenOptions};
use std::io::{ErrorKind, Read, Seek, SeekFrom, Write};
use std::ops::Range;
#[cfg(target_family = "unix")]
use std::os::unix::fs::FileExt;
#[cfg(target_family = "windows")]
use std::os::windows::fs::FileExt;
use std::sync::Arc;
use std::time::SystemTime;
use std::{collections::BTreeSet, io};
use std::{collections::VecDeque, path::PathBuf};

use async_trait::async_trait;
use bytes::Bytes;
use chrono::{DateTime, Utc};
use futures_util::{
    stream::{self, BoxStream},
    StreamExt,
};
use futures_util::{FutureExt, TryStreamExt};
use parking_lot::Mutex;
use url::Url;
use walkdir::{DirEntry, WalkDir};

use crate::{
    maybe_spawn_blocking,
    path::{absolute_path_to_url, Path},
    util::InvalidGetRange,
    Attributes, GetOptions, GetResult, GetResultPayload, ListResult, MultipartUpload, ObjectMeta,
    ObjectStore, PutMode, PutMultipartOptions, PutOptions, PutPayload, PutResult, Result,
    UploadPart,
};
use crate::{CopyMode, CopyOptions, ObjectStoreExt, RenameOptions, RenameTargetMode};
use admission::{FileAdmission, FileMoveMode, FileReaderLease, FileWriteTicket};

fn admitted_error(error: io::Error) -> crate::Error {
    crate::Error::Generic {
        store: "LocalFileSystem admitted",
        source: Box::new(error),
    }
}

fn admitted_namespace_error(
    from: &std::path::Path,
    to: &std::path::Path,
    error: io::Error,
) -> crate::Error {
    match error.kind() {
        ErrorKind::NotFound => Error::NotFound {
            path: from.to_path_buf(),
            source: error,
        }
        .into(),
        ErrorKind::AlreadyExists => Error::AlreadyExists {
            path: to.to_string_lossy().into_owned(),
            source: error,
        }
        .into(),
        _ => admitted_error(error),
    }
}

fn admitted_link(
    admission: &dyn FileAdmission,
    from: &std::path::Path,
    to: &std::path::Path,
) -> Result<()> {
    let mut ticket = admission
        .begin_link(from, to)
        .map_err(|error| admitted_namespace_error(from, to, error))?;
    ticket
        .perform_and_sync()
        .map_err(|error| admitted_namespace_error(from, to, error))
}

fn admitted_move(
    admission: &dyn FileAdmission,
    from: &std::path::Path,
    to: &std::path::Path,
    mode: FileMoveMode,
) -> Result<()> {
    let mut ticket = admission
        .begin_move(from, to, mode)
        .map_err(|error| admitted_namespace_error(from, to, error))?;
    ticket
        .perform_and_sync()
        .map_err(|error| admitted_namespace_error(from, to, error))
}

fn admitted_copy(
    admission: &dyn FileAdmission,
    from: &std::path::Path,
    to: &std::path::Path,
    mode: CopyMode,
) -> Result<()> {
    admission
        .reconcile_pending_namespace()
        .map_err(admitted_error)?;
    if from == to {
        return Err(admitted_error(io::Error::new(
            ErrorKind::InvalidInput,
            "admitted copy source equals target",
        )));
    }
    admission.prepare_parent_dirs(to).map_err(admitted_error)?;
    match mode {
        CopyMode::Create => admitted_link(admission, from, to),
        CopyMode::Overwrite => {
            let mut id = 0_u64;
            let staged = loop {
                let path = staged_upload_path(to, &id.to_string());
                match admitted_link(admission, from, &path) {
                    Ok(()) => break path,
                    Err(crate::Error::AlreadyExists { .. }) => {
                        id = id.checked_add(1).ok_or_else(|| {
                            admitted_error(io::Error::other("staging suffix overflow"))
                        })?;
                    }
                    Err(error) => return Err(error),
                }
            };
            let mut ticket =
                match admission.begin_move(&staged, to, FileMoveMode::OverwriteFromAlias) {
                    Ok(ticket) => ticket,
                    Err(error) => {
                        let primary = admitted_namespace_error(&staged, to, error);
                        return match admission.delete_file_and_sync(&staged) {
                            Ok(()) => Err(primary),
                            Err(cleanup) => Err(crate::Error::Generic {
                                store: "LocalFileSystem admitted",
                                source: Box::new(AdmittedPrepublicationCleanupError {
                                    primary,
                                    cleanup: admitted_error(cleanup),
                                }),
                            }),
                        };
                    }
                };
            ticket
                .perform_and_sync()
                .map_err(|error| admitted_namespace_error(&staged, to, error))
        }
    }
}

fn sync_parent(path: &std::path::Path) -> Result<()> {
    let parent = path
        .parent()
        .ok_or_else(|| admitted_error(io::Error::other("missing parent")))?;
    File::open(parent)
        .and_then(|file| file.sync_all())
        .map_err(admitted_error)
}

fn admitted_file_stream(
    lease: Arc<dyn FileReaderLease>,
    range: Range<u64>,
) -> BoxStream<'static, Result<Bytes>> {
    stream::try_unfold(
        (lease, range.start, range.end),
        |(lease, cursor, end)| async move {
            if cursor >= end {
                return Ok(None);
            }
            let next = cursor.saturating_add(8 * 1024).min(end);
            let (lease, bytes) = maybe_spawn_blocking(move || {
                let mut reader = lease.file().map_err(admitted_error)?;
                reader
                    .seek(SeekFrom::Start(cursor))
                    .map_err(admitted_error)?;
                let length = usize::try_from(next - cursor)
                    .map_err(|error| admitted_error(io::Error::other(error)))?;
                let mut data = vec![0; length];
                reader.read_exact(&mut data).map_err(admitted_error)?;
                Ok((lease, Bytes::from(data)))
            })
            .await?;
            Ok(Some((bytes, (lease, next, end))))
        },
    )
    .boxed()
}

fn admitted_entry_metadata(entry: &DirEntry, location: Path) -> Result<Option<ObjectMeta>> {
    if !entry.file_type().is_file() {
        return Ok(None);
    }
    let metadata = symlink_metadata(entry.path()).map_err(|source| Error::Metadata {
        source: Box::new(source),
        path: entry.path().to_string_lossy().into_owned(),
    })?;
    Ok(metadata
        .is_file()
        .then(|| convert_metadata(metadata, location)))
}

// A pre-publication failure still owns the precise FD witnessed by `created`.
// Unlink before transferring it so the binder can prove zero links and refund.
fn retire_failed_admitted_staging(
    staging: &std::path::Path,
    writer: File,
    ticket: &mut dyn FileWriteTicket,
) -> Result<()> {
    std::fs::remove_file(staging).map_err(|source| Error::UnableToDeleteFile {
        source,
        path: staging.to_path_buf(),
    })?;
    ticket
        .confirmed_safe_absent_after_writer_close(writer)
        .map_err(admitted_error)
}

#[derive(Debug)]
struct AdmittedPrepublicationCleanupError {
    primary: crate::Error,
    cleanup: crate::Error,
}

impl std::fmt::Display for AdmittedPrepublicationCleanupError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            formatter,
            "pre-publication failure: {}; staging cleanup failed: {}",
            self.primary, self.cleanup
        )
    }
}

impl std::error::Error for AdmittedPrepublicationCleanupError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(&self.primary)
    }
}

fn retire_failed_admitted_staging_with_cause(
    staging: &std::path::Path,
    writer: File,
    ticket: &mut dyn FileWriteTicket,
    primary: crate::Error,
) -> crate::Error {
    match retire_failed_admitted_staging(staging, writer, ticket) {
        Ok(()) => primary,
        Err(cleanup) => crate::Error::Generic {
            store: "LocalFileSystem admitted",
            source: Box::new(AdmittedPrepublicationCleanupError { primary, cleanup }),
        },
    }
}

/// A specialized `Error` for filesystem object store-related errors
#[derive(Debug, thiserror::Error)]
pub(crate) enum Error {
    #[error("Unable to walk dir: {}", source)]
    UnableToWalkDir { source: walkdir::Error },

    #[error("Unable to access metadata for {}: {}", path, source)]
    Metadata {
        source: Box<dyn std::error::Error + Send + Sync + 'static>,
        path: String,
    },

    #[error("Unable to copy data to file: {}", source)]
    UnableToCopyDataToFile { source: io::Error },

    #[error("Unable to rename file: {}", source)]
    UnableToRenameFile { source: io::Error },

    #[error("Unable to create dir {}: {}", path.display(), source)]
    UnableToCreateDir { source: io::Error, path: PathBuf },

    #[error("Unable to create file {}: {}", path.display(), source)]
    UnableToCreateFile { source: io::Error, path: PathBuf },

    #[error("Unable to delete file {}: {}", path.display(), source)]
    UnableToDeleteFile { source: io::Error, path: PathBuf },

    #[error("Unable to open file {}: {}", path.display(), source)]
    UnableToOpenFile { source: io::Error, path: PathBuf },

    #[error("Unable to read data from file {}: {}", path.display(), source)]
    UnableToReadBytes { source: io::Error, path: PathBuf },

    #[error("Out of range of file {}, expected: {}, actual: {}", path.display(), expected, actual)]
    OutOfRange {
        path: PathBuf,
        expected: u64,
        actual: u64,
    },

    #[error("Requested range was invalid")]
    InvalidRange { source: InvalidGetRange },

    #[error("Unable to copy file from {} to {}: {}", from.display(), to.display(), source)]
    UnableToCopyFile {
        from: PathBuf,
        to: PathBuf,
        source: io::Error,
    },

    #[error("NotFound")]
    NotFound { path: PathBuf, source: io::Error },

    #[error("Error seeking file {}: {}", path.display(), source)]
    Seek { source: io::Error, path: PathBuf },

    #[error("Unable to convert URL \"{}\" to filesystem path", url)]
    InvalidUrl { url: Url },

    #[error("AlreadyExists")]
    AlreadyExists { path: String, source: io::Error },

    #[error("Unable to canonicalize filesystem root: {}", path.display())]
    UnableToCanonicalize { path: PathBuf, source: io::Error },

    #[error("Filenames containing trailing '/#\\d+/' are not supported: {}", path)]
    InvalidPath { path: String },

    #[error("Upload aborted")]
    Aborted,
}

impl From<Error> for super::Error {
    fn from(source: Error) -> Self {
        match source {
            Error::NotFound { path, source } => Self::NotFound {
                path: path.to_string_lossy().to_string(),
                source: source.into(),
            },
            Error::AlreadyExists { path, source } => Self::AlreadyExists {
                path,
                source: source.into(),
            },
            _ => Self::Generic {
                store: "LocalFileSystem",
                source: Box::new(source),
            },
        }
    }
}

/// Local filesystem storage providing an [`ObjectStore`] interface to files on
/// local disk. Can optionally be created with a directory prefix
///
/// # Path Semantics
///
/// This implementation follows the [file URI] scheme outlined in [RFC 3986]. In
/// particular paths are delimited by `/`
///
/// [file URI]: https://en.wikipedia.org/wiki/File_URI_scheme
/// [RFC 3986]: https://www.rfc-editor.org/rfc/rfc3986
///
/// # Path Semantics
///
/// [`LocalFileSystem`] will expose the path semantics of the underlying filesystem, which may
/// have additional restrictions beyond those enforced by [`Path`].
///
/// For example:
///
/// * Windows forbids certain filenames, e.g. `COM0`,
/// * Windows forbids folders with trailing `.`
/// * Windows forbids certain ASCII characters, e.g. `<` or `|`
/// * OS X forbids filenames containing `:`
/// * Leading `-` are discouraged on Unix systems where they may be interpreted as CLI flags
/// * Filesystems may have restrictions on the maximum path or path segment length
/// * Filesystem support for non-ASCII characters is inconsistent
///
/// Additionally some filesystems, such as NTFS, are case-insensitive, whilst others like
/// FAT don't preserve case at all. Further some filesystems support non-unicode character
/// sequences, such as unpaired UTF-16 surrogates, and [`LocalFileSystem`] will error on
/// encountering such sequences.
///
/// Finally, filenames matching the regex `/.*#\d+/`, e.g. `foo.parquet#123`, are not supported
/// by [`LocalFileSystem`] as they are used to provide atomic writes. Such files will be ignored
/// for listing operations, and attempting to address such a file will error.
///
/// # Tokio Compatibility
///
/// Tokio discourages performing blocking IO on a tokio worker thread, however,
/// no major operating systems have stable async file APIs. Therefore if called from
/// a tokio context, this will use [`tokio::runtime::Handle::spawn_blocking`] to dispatch
/// IO to a blocking thread pool, much like `tokio::fs` does under-the-hood.
///
/// If not called from a tokio context, this will perform IO on the current thread with
/// no additional complexity or overheads
///
/// # Symlinks
///
/// [`LocalFileSystem`] will follow symlinks as normal, however, it is worth noting:
///
/// * Broken symlinks will be silently ignored by listing operations
/// * No effort is made to prevent breaking symlinks when deleting files
/// * Symlinks that resolve to paths outside the root **will** be followed
/// * Mutating a file through one or more symlinks will mutate the underlying file
/// * Deleting a path that resolves to a symlink will only delete the symlink
///
/// # Cross-Filesystem Copy
///
/// [`LocalFileSystem::copy_opts`] is implemented using [`std::fs::hard_link`], and therefore
/// does not support copying across filesystem boundaries.
///
#[derive(Clone, Debug)]
pub struct LocalFileSystem {
    config: Arc<Config>,
    // if you want to delete empty directories when deleting files
    automatic_cleanup: bool,
    admission: Option<Arc<dyn FileAdmission>>,
}

#[derive(Debug)]
struct Config {
    root: Url,
}

impl std::fmt::Display for LocalFileSystem {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "LocalFileSystem({})", self.config.root)
    }
}

impl Default for LocalFileSystem {
    fn default() -> Self {
        Self::new()
    }
}

impl LocalFileSystem {
    /// Install the same per-Session admission used by Lance's optimized local
    /// writer. The local store owns its reader FD and routes direct namespace
    /// mutations through this authority; the Lance wrapper uses the same owner.
    pub fn with_admission(mut self, admission: Arc<dyn FileAdmission>) -> Self {
        self.admission = Some(admission);
        self
    }

    /// Create new filesystem storage with no prefix
    pub fn new() -> Self {
        Self {
            config: Arc::new(Config {
                root: Url::parse("file:///").unwrap(),
            }),
            automatic_cleanup: false,
            admission: None,
        }
    }

    /// Create new filesystem storage with `prefix` applied to all paths
    ///
    /// Returns an error if the path does not exist
    ///
    pub fn new_with_prefix(prefix: impl AsRef<std::path::Path>) -> Result<Self> {
        let path = std::fs::canonicalize(&prefix).map_err(|source| {
            let path = prefix.as_ref().into();
            Error::UnableToCanonicalize { source, path }
        })?;

        Ok(Self {
            config: Arc::new(Config {
                root: absolute_path_to_url(path)?,
            }),
            automatic_cleanup: false,
            admission: None,
        })
    }

    /// Return an absolute filesystem path of the given file location
    pub fn path_to_filesystem(&self, location: &Path) -> Result<PathBuf> {
        self.config.path_to_filesystem(location)
    }

    /// Enable automatic cleanup of empty directories when deleting files
    pub fn with_automatic_cleanup(mut self, automatic_cleanup: bool) -> Self {
        self.automatic_cleanup = automatic_cleanup;
        self
    }
}

impl Config {
    /// Return an absolute filesystem path of the given location
    fn prefix_to_filesystem(&self, location: &Path) -> Result<PathBuf> {
        let mut url = self.root.clone();
        url.path_segments_mut()
            .expect("url path")
            // technically not necessary as Path ignores empty segments
            // but avoids creating paths with "//" which look odd in error messages.
            .pop_if_empty()
            .extend(location.parts());

        url.to_file_path()
            .map_err(|_| Error::InvalidUrl { url }.into())
    }

    /// Return an absolute filesystem path of the given file location
    fn path_to_filesystem(&self, location: &Path) -> Result<PathBuf> {
        if !is_valid_file_path(location) {
            let path = location.as_ref().into();
            let error = Error::InvalidPath { path };
            return Err(error.into());
        }

        let path = self.prefix_to_filesystem(location)?;

        #[cfg(target_os = "windows")]
        let path = {
            let path = path.to_string_lossy();

            // Assume the first char is the drive letter and the next is a colon.
            let mut out = String::new();
            let drive = &path[..2]; // The drive letter and colon (e.g., "C:")
            let filepath = &path[2..].replace(':', "%3A"); // Replace subsequent colons
            out.push_str(drive);
            out.push_str(filepath);
            PathBuf::from(out)
        };

        Ok(path)
    }

    /// Resolves the provided absolute filesystem path to a [`Path`] prefix
    fn filesystem_to_path(&self, location: &std::path::Path) -> Result<Path> {
        Ok(Path::from_absolute_path_with_base(
            location,
            Some(&self.root),
        )?)
    }
}

fn is_valid_file_path(path: &Path) -> bool {
    match path.filename() {
        Some(p) => match p.split_once('#') {
            Some((_, suffix)) if !suffix.is_empty() => {
                // Valid if contains non-digits
                !suffix.as_bytes().iter().all(|x| x.is_ascii_digit())
            }
            _ => true,
        },
        None => false,
    }
}

#[async_trait]
impl ObjectStore for LocalFileSystem {
    async fn put_opts(
        &self,
        location: &Path,
        payload: PutPayload,
        opts: PutOptions,
    ) -> Result<PutResult> {
        if matches!(opts.mode, PutMode::Update(_)) {
            return Err(crate::Error::NotImplemented {
                operation: "`put_opts` with mode `PutMode::Update`".into(),
                implementer: self.to_string(),
            });
        }

        if !opts.attributes.is_empty() {
            return Err(crate::Error::NotImplemented {
                operation: "`put_opts` with `opts.attributes` specified".into(),
                implementer: self.to_string(),
            });
        }

        let path = self.path_to_filesystem(location)?;
        if let Some(admission) = &self.admission {
            admission
                .reconcile_pending_namespace()
                .map_err(admitted_error)?;
            admission
                .prepare_parent_dirs(&path)
                .map_err(admitted_error)?;
        }
        let mut ticket = self
            .admission
            .as_ref()
            .map(|admission| admission.begin_create(&path))
            .transpose()
            .map_err(admitted_error)?;
        let admission = self.admission.clone();
        maybe_spawn_blocking(move || {
            let (mut file, staging_path) =
                new_staged_upload(&path, ticket.as_mut(), admission.as_deref())?;
            let mut e_tag = None;
            let mut attempted = 0_u64;

            let err = match payload.iter().try_for_each(|x| {
                attempted = attempted
                    .checked_add(x.len() as u64)
                    .ok_or_else(|| io::Error::other("local put length overflow"))?;
                if let Some(ticket) = ticket.as_deref_mut() {
                    ticket.reserve_end(attempted)?;
                }
                file.write_all(x)
            }) {
                Ok(_) => {
                    let metadata = match file.metadata() {
                        Ok(metadata) => metadata,
                        Err(source) => {
                            let primary = Error::Metadata {
                                source: source.into(),
                                path: path.to_string_lossy().to_string(),
                            }
                            .into();
                            return Err(match ticket.as_deref_mut() {
                                Some(ticket) => retire_failed_admitted_staging_with_cause(
                                    &staging_path,
                                    file,
                                    ticket,
                                    primary,
                                ),
                                None => primary,
                            });
                        }
                    };
                    e_tag = Some(get_etag(&metadata));
                    if let Some(ticket) = ticket.as_deref_mut() {
                        if let Err(source) = file.sync_all() {
                            return Err(retire_failed_admitted_staging_with_cause(
                                &staging_path,
                                file,
                                ticket,
                                admitted_error(source),
                            ));
                        }
                        if let Err(error) = ticket.before_replace(&path) {
                            return Err(retire_failed_admitted_staging_with_cause(
                                &staging_path,
                                file,
                                ticket,
                                admitted_error(error),
                            ));
                        }
                    }
                    // `created` authenticated the original file descriptor. A
                    // clone may be closed as an auxiliary writer, but must
                    // never be presented as that original final writer.
                    let mut auxiliary_file = if let Some(ticket) = ticket.as_deref_mut() {
                        Some(match file.try_clone() {
                            Ok(auxiliary) => auxiliary,
                            Err(source) => {
                                return Err(retire_failed_admitted_staging_with_cause(
                                    &staging_path,
                                    file,
                                    ticket,
                                    admitted_error(source),
                                ));
                            }
                        })
                    } else {
                        None
                    };
                    let mut original_file = Some(file);
                    let mut published = false;
                    let err = match opts.mode {
                        PutMode::Overwrite => {
                            if let Some(ticket) = ticket.as_deref_mut() {
                                let auxiliary = auxiliary_file.take().ok_or_else(|| {
                                    admitted_error(io::Error::other("put auxiliary FD missing"))
                                })?;
                                ticket
                                    .confirmed_auxiliary_writer_fd_retired(auxiliary)
                                    .map_err(admitted_error)?;
                            } else {
                                // Some FUSE stores, including Blobfuse, upload
                                // only after close. Preserve the upstream
                                // close-before-rename behavior for that store.
                                drop(original_file.take().ok_or_else(|| {
                                    admitted_error(io::Error::other("put staging FD missing"))
                                })?);
                            }
                            match std::fs::rename(&staging_path, &path) {
                                Ok(()) => {
                                    published = true;
                                    None
                                }
                                Err(source) => Some(Error::UnableToRenameFile { source }),
                            }
                        }
                        PutMode::Create => match std::fs::hard_link(&staging_path, &path) {
                            Ok(()) => {
                                published = true;
                                match std::fs::remove_file(&staging_path) {
                                    Ok(()) => None,
                                    Err(source) if ticket.is_none() => {
                                        // Preserve the unqualified upstream best-effort cleanup.
                                        let _ = source;
                                        None
                                    }
                                    Err(source) => Some(Error::UnableToRenameFile { source }),
                                }
                            }
                            Err(source) => match source.kind() {
                                ErrorKind::AlreadyExists => Some(Error::AlreadyExists {
                                    path: path.to_string_lossy().into_owned(),
                                    source,
                                }),
                                _ => Some(Error::UnableToRenameFile { source }),
                            },
                        },
                        PutMode::Update(_) => unreachable!(),
                    };
                    if let Some(error) = err {
                        if let Some(ticket) = ticket.as_deref_mut() {
                            if !published {
                                if let Some(auxiliary) = auxiliary_file.take() {
                                    ticket
                                        .confirmed_auxiliary_writer_fd_retired(auxiliary)
                                        .map_err(admitted_error)?;
                                }
                                let original = original_file.take().ok_or_else(|| {
                                    admitted_error(io::Error::other("admitted put FD missing"))
                                })?;
                                return Err(retire_failed_admitted_staging_with_cause(
                                    &staging_path,
                                    original,
                                    ticket,
                                    error.into(),
                                ));
                            }
                            // A successful hard link already published a final
                            // path; a failed staging unlink cannot authorize
                            // absent-file settlement.
                        }
                        let _ = std::fs::remove_file(&staging_path);
                        return Err(error.into());
                    }
                    if let Some(ticket) = ticket.as_deref_mut() {
                        sync_parent(&path)?;
                        let original = original_file.as_ref().ok_or_else(|| {
                            admitted_error(io::Error::other("admitted put FD missing"))
                        })?;
                        ticket
                            .replaced_and_synced(original, &path)
                            .map_err(admitted_error)?;
                        if let Some(auxiliary) = auxiliary_file.take() {
                            ticket
                                .confirmed_auxiliary_writer_fd_retired(auxiliary)
                                .map_err(admitted_error)?;
                        }
                        let original = original_file.take().ok_or_else(|| {
                            admitted_error(io::Error::other("admitted put FD missing"))
                        })?;
                        ticket
                            .confirmed_writer_fd_retired(original)
                            .map_err(admitted_error)?;
                    }
                    None
                }
                Err(source) => {
                    if let Some(ticket) = ticket.as_deref_mut() {
                        return Err(retire_failed_admitted_staging_with_cause(
                            &staging_path,
                            file,
                            ticket,
                            Error::UnableToCopyDataToFile { source }.into(),
                        ));
                    }
                    Some(Error::UnableToCopyDataToFile { source })
                }
            };

            if let Some(err) = err {
                // The exact writer FD may still be open. The ticket remains
                // uncertain until a later authenticated physical witness.
                let _ = std::fs::remove_file(&staging_path);
                return Err(err.into());
            }

            Ok(PutResult {
                e_tag,
                version: None,
            })
        })
        .await
    }

    async fn put_multipart_opts(
        &self,
        location: &Path,
        opts: PutMultipartOptions,
    ) -> Result<Box<dyn MultipartUpload>> {
        if !opts.attributes.is_empty() {
            return Err(crate::Error::NotImplemented {
                operation: "`put_multipart_opts` with `opts.attributes` specified".into(),
                implementer: self.to_string(),
            });
        }

        let dest = self.path_to_filesystem(location)?;
        if let Some(admission) = &self.admission {
            admission
                .reconcile_pending_namespace()
                .map_err(admitted_error)?;
            admission
                .prepare_parent_dirs(&dest)
                .map_err(admitted_error)?;
        }
        let mut ticket = self
            .admission
            .as_ref()
            .map(|admission| admission.begin_create(&dest))
            .transpose()
            .map_err(admitted_error)?;
        let (file, src) = new_staged_upload(&dest, ticket.as_mut(), self.admission.as_deref())?;
        Ok(Box::new(LocalUpload::new(src, dest, file, ticket)))
    }

    async fn get_opts(&self, location: &Path, options: GetOptions) -> Result<GetResult> {
        let location = location.clone();
        let path = self.path_to_filesystem(&location)?;
        if let Some(admission) = &self.admission {
            admission
                .reconcile_pending_namespace()
                .map_err(admitted_error)?;
            let admission = Arc::clone(admission);
            return maybe_spawn_blocking(move || {
                let mut reader_open = admission.begin_reader_open(&path).map_err(|error| {
                    if error.kind() == ErrorKind::NotFound {
                        Error::NotFound {
                            path: path.clone(),
                            source: error,
                        }
                        .into()
                    } else {
                        admitted_error(error)
                    }
                })?;
                let file = open_file(&path)?;
                let lease = reader_open.opened_owned(file).map_err(admitted_error)?;
                drop(reader_open);
                let metadata = open_metadata(lease.file().map_err(admitted_error)?, &path)?;
                let meta = convert_metadata(metadata, location);
                options.check_preconditions(&meta)?;
                let range = match options.range {
                    Some(range) => range
                        .as_range(meta.size)
                        .map_err(|source| Error::InvalidRange { source })?,
                    None => 0..meta.size,
                };
                Ok(GetResult {
                    payload: GetResultPayload::Stream(admitted_file_stream(lease, range.clone())),
                    attributes: Attributes::default(),
                    range,
                    meta,
                })
            })
            .await;
        }
        maybe_spawn_blocking(move || {
            let file = open_file(&path)?;
            let metadata = open_metadata(&file, &path)?;
            let meta = convert_metadata(metadata, location);
            options.check_preconditions(&meta)?;

            let range = match options.range {
                Some(r) => r
                    .as_range(meta.size)
                    .map_err(|source| Error::InvalidRange { source })?,
                None => 0..meta.size,
            };

            Ok(GetResult {
                payload: GetResultPayload::File(file, path),
                attributes: Attributes::default(),
                range,
                meta,
            })
        })
        .await
    }

    async fn get_ranges(&self, location: &Path, ranges: &[Range<u64>]) -> Result<Vec<Bytes>> {
        if let Some(admission) = &self.admission {
            admission
                .reconcile_pending_namespace()
                .map_err(admitted_error)?;
            let mut result = Vec::with_capacity(ranges.len());
            for range in ranges {
                result.push(self.get_range(location, range.clone()).await?);
            }
            return Ok(result);
        }
        let path = self.path_to_filesystem(location)?;
        let ranges = ranges.to_vec();
        maybe_spawn_blocking(move || {
            // Vectored IO might be faster
            // We do not read the metadata here, but error in `read_range` if necessary
            let mut file = File::open(&path).map_err(|e| map_open_error(e, &path))?;
            ranges
                .into_iter()
                .map(|r| read_range(&mut file, &path, r))
                .collect()
        })
        .await
    }

    fn delete_stream(
        &self,
        locations: BoxStream<'static, Result<Path>>,
    ) -> BoxStream<'static, Result<Path>> {
        if let Some(admission) = &self.admission {
            if !self.automatic_cleanup {
                let admission = Arc::clone(admission);
                let config = Arc::clone(&self.config);
                return locations
                    .map(move |location| {
                        let admission = Arc::clone(&admission);
                        let config = Arc::clone(&config);
                        maybe_spawn_blocking(move || {
                            let location = location?;
                            let path = config.path_to_filesystem(&location)?;
                            admission
                                .reconcile_pending_namespace()
                                .map_err(admitted_error)?;
                            admission.delete_file_and_sync(&path).map_err(|error| {
                                if error.kind() == ErrorKind::NotFound {
                                    Error::NotFound {
                                        path,
                                        source: error,
                                    }
                                    .into()
                                } else {
                                    admitted_error(error)
                                }
                            })?;
                            Ok(location)
                        })
                    })
                    .buffered(10)
                    .boxed();
            }
            return locations
                .map(|location| {
                    location.and_then(|_path| {
                        Err(admitted_error(io::Error::new(
                            ErrorKind::Unsupported,
                            "admitted local automatic directory cleanup is unsupported",
                        )))
                    })
                })
                .boxed();
        }
        let config = Arc::clone(&self.config);
        let automatic_cleanup = self.automatic_cleanup;
        locations
            .map(move |location| {
                let config = Arc::clone(&config);
                maybe_spawn_blocking(move || {
                    let location = location?;
                    Self::delete_location(config, automatic_cleanup, &location)?;
                    Ok(location)
                })
            })
            .buffered(10)
            .boxed()
    }

    fn list(&self, prefix: Option<&Path>) -> BoxStream<'static, Result<ObjectMeta>> {
        if let Some(admission) = &self.admission {
            if let Err(error) = admission.reconcile_pending_namespace() {
                return futures_util::future::ready(Err(admitted_error(error)))
                    .into_stream()
                    .boxed();
            }
        }
        Self::list_with_maybe_offset(
            Arc::clone(&self.config),
            prefix,
            None,
            self.admission.clone(),
        )
    }

    fn list_with_offset(
        &self,
        prefix: Option<&Path>,
        offset: &Path,
    ) -> BoxStream<'static, Result<ObjectMeta>> {
        if let Some(admission) = &self.admission {
            if let Err(error) = admission.reconcile_pending_namespace() {
                return futures_util::future::ready(Err(admitted_error(error)))
                    .into_stream()
                    .boxed();
            }
        }
        Self::list_with_maybe_offset(
            Arc::clone(&self.config),
            prefix,
            Some(offset),
            self.admission.clone(),
        )
    }

    async fn list_with_delimiter(&self, prefix: Option<&Path>) -> Result<ListResult> {
        if let Some(admission) = &self.admission {
            admission
                .reconcile_pending_namespace()
                .map_err(admitted_error)?;
        }
        let config = Arc::clone(&self.config);
        let admission = self.admission.clone();

        let prefix = prefix.cloned().unwrap_or_default();
        let resolved_prefix = config.prefix_to_filesystem(&prefix)?;
        if let Some(admission) = &admission {
            admission
                .validate_list_scope(&resolved_prefix)
                .map_err(admitted_error)?;
        }

        maybe_spawn_blocking(move || {
            let walkdir = WalkDir::new(&resolved_prefix)
                .min_depth(1)
                .max_depth(1)
                .follow_links(admission.is_none());

            let mut common_prefixes = BTreeSet::new();
            let mut objects = Vec::new();

            for entry_res in walkdir.into_iter().map(convert_walkdir_result) {
                if let Some(entry) = entry_res? {
                    if admission.is_some() && entry.file_type().is_symlink() {
                        continue;
                    }
                    let is_directory = entry.file_type().is_dir();
                    let entry_location = config.filesystem_to_path(entry.path())?;
                    if !is_directory && !is_valid_file_path(&entry_location) {
                        continue;
                    }

                    let mut parts = match entry_location.prefix_match(&prefix) {
                        Some(parts) => parts,
                        None => continue,
                    };

                    let common_prefix = match parts.next() {
                        Some(p) => p,
                        None => continue,
                    };

                    drop(parts);

                    if is_directory {
                        if let Some(admission) = &admission {
                            admission
                                .validate_list_scope(entry.path())
                                .map_err(admitted_error)?;
                        }
                        common_prefixes.insert(prefix.clone().join(common_prefix));
                    } else if let Some(metadata) = match &admission {
                        Some(_admission) => admitted_entry_metadata(&entry, entry_location)?,
                        None => convert_entry(entry, entry_location)?,
                    } {
                        objects.push(metadata);
                    }
                }
            }

            Ok(ListResult {
                common_prefixes: common_prefixes.into_iter().collect(),
                objects,
            })
        })
        .await
    }

    async fn copy_opts(&self, from: &Path, to: &Path, options: CopyOptions) -> Result<()> {
        if let Some(admission) = &self.admission {
            let from = self.path_to_filesystem(from)?;
            let to = self.path_to_filesystem(to)?;
            let admission = Arc::clone(admission);
            return maybe_spawn_blocking(move || {
                admitted_copy(admission.as_ref(), &from, &to, options.mode)
            })
            .await;
        }
        let CopyOptions {
            mode,
            extensions: _,
        } = options;

        let from = self.path_to_filesystem(from)?;
        let to = self.path_to_filesystem(to)?;

        match mode {
            CopyMode::Overwrite => {
                let mut id = 0;
                // In order to make this atomic we:
                //
                // - hard link to a hidden temporary file
                // - atomically rename this temporary file into place
                //
                // This is necessary because hard_link returns an error if the destination already exists
                maybe_spawn_blocking(move || {
                    loop {
                        let staged = staged_upload_path(&to, &id.to_string());
                        match std::fs::hard_link(&from, &staged) {
                            Ok(_) => {
                                return std::fs::rename(&staged, &to).map_err(|source| {
                                    let _ = std::fs::remove_file(&staged); // Attempt to clean up
                                    Error::UnableToCopyFile { from, to, source }.into()
                                });
                            }
                            Err(source) => match source.kind() {
                                ErrorKind::AlreadyExists => id += 1,
                                ErrorKind::NotFound => match from.exists() {
                                    true => create_parent_dirs(&to, source)?,
                                    false => {
                                        return Err(Error::NotFound { path: from, source }.into());
                                    }
                                },
                                _ => {
                                    return Err(Error::UnableToCopyFile { from, to, source }.into());
                                }
                            },
                        }
                    }
                })
                .await
            }
            CopyMode::Create => {
                maybe_spawn_blocking(move || loop {
                    match std::fs::hard_link(&from, &to) {
                        Ok(_) => return Ok(()),
                        Err(source) => match source.kind() {
                            ErrorKind::AlreadyExists => {
                                return Err(Error::AlreadyExists {
                                    path: to.to_str().unwrap().to_string(),
                                    source,
                                }
                                .into());
                            }
                            ErrorKind::NotFound => match from.exists() {
                                true => create_parent_dirs(&to, source)?,
                                false => {
                                    return Err(Error::NotFound { path: from, source }.into());
                                }
                            },
                            _ => {
                                return Err(Error::UnableToCopyFile { from, to, source }.into());
                            }
                        },
                    }
                })
                .await
            }
        }
    }

    async fn rename_opts(&self, from: &Path, to: &Path, options: RenameOptions) -> Result<()> {
        if let Some(admission) = &self.admission {
            let from = self.path_to_filesystem(from)?;
            let to = self.path_to_filesystem(to)?;
            let admission = Arc::clone(admission);
            return maybe_spawn_blocking(move || {
                admission.prepare_parent_dirs(&to).map_err(admitted_error)?;
                if from == to {
                    return Err(admitted_error(io::Error::new(
                        ErrorKind::InvalidInput,
                        "admitted rename source equals target",
                    )));
                }
                let mode = match options.target_mode {
                    RenameTargetMode::Create => FileMoveMode::Create,
                    RenameTargetMode::Overwrite => FileMoveMode::Overwrite,
                };
                admitted_move(admission.as_ref(), &from, &to, mode)
            })
            .await;
        }
        let RenameOptions {
            target_mode,
            extensions,
        } = options;

        match target_mode {
            // optimized implementation
            RenameTargetMode::Overwrite => {
                let from = self.path_to_filesystem(from)?;
                let to = self.path_to_filesystem(to)?;
                maybe_spawn_blocking(move || loop {
                    match std::fs::rename(&from, &to) {
                        Ok(_) => return Ok(()),
                        Err(source) => match source.kind() {
                            ErrorKind::NotFound => match from.exists() {
                                true => create_parent_dirs(&to, source)?,
                                false => {
                                    return Err(Error::NotFound { path: from, source }.into());
                                }
                            },
                            _ => {
                                return Err(Error::UnableToCopyFile { from, to, source }.into());
                            }
                        },
                    }
                })
                .await
            }
            // fall-back to copy & delete
            RenameTargetMode::Create => {
                self.copy_opts(
                    from,
                    to,
                    CopyOptions {
                        mode: CopyMode::Create,
                        extensions,
                    },
                )
                .await?;
                self.delete(from).await?;
                Ok(())
            }
        }
    }
}

impl LocalFileSystem {
    fn delete_location(
        config: Arc<Config>,
        automatic_cleanup: bool,
        location: &Path,
    ) -> Result<()> {
        let path = config.path_to_filesystem(location)?;
        if let Err(e) = std::fs::remove_file(&path) {
            Err(match e.kind() {
                ErrorKind::NotFound => Error::NotFound { path, source: e }.into(),
                _ => Error::UnableToDeleteFile { path, source: e }.into(),
            })
        } else if automatic_cleanup {
            let root = &config.root;
            let root = root
                .to_file_path()
                .map_err(|_| Error::InvalidUrl { url: root.clone() })?;

            // here we will try to traverse up and delete an empty dir if possible until we reach the root or get an error
            let mut parent = path.parent();

            while let Some(loc) = parent {
                if loc != root && std::fs::remove_dir(loc).is_ok() {
                    parent = loc.parent();
                } else {
                    break;
                }
            }

            Ok(())
        } else {
            Ok(())
        }
    }

    fn list_with_maybe_offset(
        config: Arc<Config>,
        prefix: Option<&Path>,
        maybe_offset: Option<&Path>,
        admission: Option<Arc<dyn FileAdmission>>,
    ) -> BoxStream<'static, Result<ObjectMeta>> {
        let root_path = match prefix {
            Some(prefix) => match config.prefix_to_filesystem(prefix) {
                Ok(path) => path,
                Err(e) => return futures_util::future::ready(Err(e)).into_stream().boxed(),
            },
            None => config.root.to_file_path().unwrap(),
        };
        if let Some(admission) = &admission {
            if let Err(error) = admission.validate_list_scope(&root_path) {
                return futures_util::future::ready(Err(admitted_error(error)))
                    .into_stream()
                    .boxed();
            }
        }

        let walkdir = WalkDir::new(root_path)
            // Don't include the root directory itself
            .min_depth(1)
            .follow_links(admission.is_none());

        let maybe_offset = maybe_offset.cloned();

        let s = walkdir.into_iter().flat_map(move |result_dir_entry| {
            // Apply offset filter before proceeding, to reduce statx file system calls
            // This matters for NFS mounts
            if let (Some(offset), Ok(entry)) = (maybe_offset.as_ref(), result_dir_entry.as_ref()) {
                let location = config.filesystem_to_path(entry.path());
                match location {
                    Ok(path) if path <= *offset => return None,
                    Err(e) => return Some(Err(e)),
                    _ => {}
                }
            }

            let entry = match convert_walkdir_result(result_dir_entry).transpose()? {
                Ok(entry) => entry,
                Err(e) => return Some(Err(e)),
            };

            if admission.as_ref().map_or_else(
                || !entry.path().is_file(),
                |_admission| !entry.file_type().is_file(),
            ) {
                return None;
            }

            match config.filesystem_to_path(entry.path()) {
                Ok(path) => match is_valid_file_path(&path) {
                    true => match &admission {
                        Some(_admission) => admitted_entry_metadata(&entry, path).transpose(),
                        None => convert_entry(entry, path).transpose(),
                    },
                    false => None,
                },
                Err(e) => Some(Err(e)),
            }
        });

        // If no tokio context, return iterator directly as no
        // need to perform chunked spawn_blocking reads
        if tokio::runtime::Handle::try_current().is_err() {
            return futures_util::stream::iter(s).boxed();
        }

        // Otherwise list in batches of CHUNK_SIZE
        const CHUNK_SIZE: usize = 1024;

        let buffer = VecDeque::with_capacity(CHUNK_SIZE);
        futures_util::stream::try_unfold((s, buffer), |(mut s, mut buffer)| async move {
            if buffer.is_empty() {
                (s, buffer) = tokio::task::spawn_blocking(move || {
                    for _ in 0..CHUNK_SIZE {
                        match s.next() {
                            Some(r) => buffer.push_back(r),
                            None => break,
                        }
                    }
                    (s, buffer)
                })
                .await?;
            }

            match buffer.pop_front() {
                Some(Err(e)) => Err(e),
                Some(Ok(meta)) => Ok(Some((meta, (s, buffer)))),
                None => Ok(None),
            }
        })
        .boxed()
    }
}

/// Creates the parent directories of `path` or returns an error based on `source` if no parent
fn create_parent_dirs(path: &std::path::Path, source: io::Error) -> Result<()> {
    let parent = path.parent().ok_or_else(|| {
        let path = path.to_path_buf();
        Error::UnableToCreateFile { path, source }
    })?;

    std::fs::create_dir_all(parent).map_err(|source| {
        let path = parent.into();
        Error::UnableToCreateDir { source, path }
    })?;
    Ok(())
}

/// Generates a unique file path `{base}#{suffix}`, returning the opened `File` and `path`
///
/// Creates any directories if necessary
fn new_staged_upload(
    base: &std::path::Path,
    mut ticket: Option<&mut Box<dyn FileWriteTicket>>,
    admission: Option<&dyn FileAdmission>,
) -> Result<(File, PathBuf)> {
    let mut multipart_id = 1;
    loop {
        let suffix = multipart_id.to_string();
        let path = staged_upload_path(base, &suffix);
        if let Some(admission) = admission {
            admission
                .reconcile_pending_namespace()
                .map_err(admitted_error)?;
            if path.exists() {
                multipart_id += 1;
                continue;
            }
        }
        if let Some(ticket) = ticket.as_deref_mut() {
            ticket.declare_temp_path(&path).map_err(admitted_error)?;
        }
        let mut options = OpenOptions::new();
        if let Some(ticket) = ticket.as_deref_mut() {
            ticket.creation_started().map_err(admitted_error)?;
        }
        match options.read(true).write(true).create_new(true).open(&path) {
            Ok(f) => {
                if let Some(ticket) = ticket.as_deref_mut() {
                    ticket.created(&f, &path).map_err(admitted_error)?;
                }
                return Ok((f, path));
            }
            Err(source) => match source.kind() {
                ErrorKind::AlreadyExists if ticket.is_none() => multipart_id += 1,
                ErrorKind::NotFound if ticket.is_none() => create_parent_dirs(&path, source)?,
                _ => return Err(Error::UnableToOpenFile { source, path }.into()),
            },
        }
    }
}

/// Returns the unique upload for the given path and suffix
fn staged_upload_path(dest: &std::path::Path, suffix: &str) -> PathBuf {
    let mut staging_path = dest.as_os_str().to_owned();
    staging_path.push("#");
    staging_path.push(suffix);
    staging_path.into()
}

#[derive(Debug)]
struct LocalUpload {
    /// The upload state
    state: Arc<UploadState>,
    /// The location of the temporary file
    src: Option<PathBuf>,
    /// The next offset to write into the file
    offset: u64,
    ticket: Option<Box<dyn FileWriteTicket>>,
}

#[derive(Debug)]
struct UploadState {
    dest: PathBuf,
    file: Mutex<Option<File>>,
}

fn retire_aborted_local_upload(
    state: &UploadState,
    src: &std::path::Path,
    ticket: Option<&mut Box<dyn FileWriteTicket>>,
) -> Result<()> {
    // Hold the mutex across unlink and FD transfer. Any queued part writer
    // can only see `None` after this critical section and return Aborted.
    let mut locked = state.file.lock();
    let writer = locked.take().ok_or(Error::Aborted)?;
    if let Some(ticket) = ticket {
        std::fs::remove_file(src).map_err(|source| Error::UnableToDeleteFile {
            source,
            path: src.to_path_buf(),
        })?;
        ticket
            .confirmed_safe_absent_after_writer_close(writer)
            .map_err(admitted_error)?;
    } else {
        // Keep unqualified local/FUSE close-before-unlink behavior.
        drop(writer);
        std::fs::remove_file(src).map_err(|source| Error::UnableToDeleteFile {
            source,
            path: src.to_path_buf(),
        })?;
    }
    Ok(())
}

impl LocalUpload {
    pub(crate) fn new(
        src: PathBuf,
        dest: PathBuf,
        file: File,
        ticket: Option<Box<dyn FileWriteTicket>>,
    ) -> Self {
        Self {
            state: Arc::new(UploadState {
                dest,
                file: Mutex::new(Some(file)),
            }),
            src: Some(src),
            offset: 0,
            ticket,
        }
    }
}

#[async_trait]
impl MultipartUpload for LocalUpload {
    fn put_part(&mut self, data: PutPayload) -> UploadPart {
        let offset = self.offset;
        let Some(candidate_end) = offset.checked_add(data.content_length() as u64) else {
            return async {
                Err(admitted_error(io::Error::other(
                    "multipart offset overflow",
                )))
            }
            .boxed();
        };
        if let Some(ticket) = self.ticket.as_deref_mut() {
            if let Err(error) = ticket.reserve_end(candidate_end) {
                return async move { Err(admitted_error(error)) }.boxed();
            }
        }
        self.offset = candidate_end;

        let s = Arc::clone(&self.state);
        maybe_spawn_blocking(move || {
            let mut locked = s.file.lock();
            let file = locked.as_mut().ok_or(Error::Aborted)?;
            file.seek(SeekFrom::Start(offset)).map_err(|source| {
                let path = s.dest.clone();
                Error::Seek { source, path }
            })?;

            data.iter()
                .try_for_each(|x| file.write_all(x))
                .map_err(|source| Error::UnableToCopyDataToFile { source })?;

            Ok(())
        })
        .boxed()
    }

    async fn complete(&mut self) -> Result<PutResult> {
        let src = self.src.take().ok_or(Error::Aborted)?;
        let s = Arc::clone(&self.state);
        let mut ticket = self.ticket.take();
        maybe_spawn_blocking(move || {
            // Ensure no inflight writes
            let file = s.file.lock().take().ok_or(Error::Aborted)?;
            if let Some(ticket) = ticket.as_deref_mut() {
                if let Err(source) = file.sync_all() {
                    return Err(retire_failed_admitted_staging_with_cause(
                        &src,
                        file,
                        ticket,
                        admitted_error(source),
                    ));
                }
                if let Err(source) = ticket.before_replace(&s.dest) {
                    return Err(retire_failed_admitted_staging_with_cause(
                        &src,
                        file,
                        ticket,
                        admitted_error(source),
                    ));
                }
            }
            if let Err(source) = std::fs::rename(&src, &s.dest) {
                let primary = Error::UnableToRenameFile { source }.into();
                return Err(match ticket.as_deref_mut() {
                    Some(ticket) => {
                        retire_failed_admitted_staging_with_cause(&src, file, ticket, primary)
                    }
                    None => primary,
                });
            }
            let metadata = file.metadata().map_err(|e| Error::Metadata {
                source: e.into(),
                path: src.to_string_lossy().to_string(),
            })?;

            if let Some(ticket) = ticket.as_deref_mut() {
                sync_parent(&s.dest)?;
                ticket
                    .replaced_and_synced(&file, &s.dest)
                    .map_err(admitted_error)?;
                ticket
                    .confirmed_writer_fd_retired(file)
                    .map_err(admitted_error)?;
            } else {
                drop(file);
            }
            Ok(PutResult {
                e_tag: Some(get_etag(&metadata)),
                version: None,
            })
        })
        .await
    }

    async fn abort(&mut self) -> Result<()> {
        let src = self.src.take().ok_or(Error::Aborted)?;
        let mut ticket = self.ticket.take();
        let s = Arc::clone(&self.state);
        maybe_spawn_blocking(move || retire_aborted_local_upload(&s, &src, ticket.as_mut())).await
    }
}

impl Drop for LocalUpload {
    fn drop(&mut self) {
        if let Some(src) = self.src.take() {
            let mut ticket = self.ticket.take();
            let state = Arc::clone(&self.state);
            // Drop cannot report an error. A failed physical witness leaves
            // the ticket uncertain; it must never create a false refund.
            match tokio::runtime::Handle::try_current() {
                Ok(runtime) => drop(runtime.spawn_blocking(move || {
                    let _result = retire_aborted_local_upload(&state, &src, ticket.as_mut());
                })),
                Err(_) => {
                    let _result = retire_aborted_local_upload(&state, &src, ticket.as_mut());
                }
            };
        }
    }
}

pub(crate) fn chunked_stream(
    mut file: File,
    path: PathBuf,
    range: Range<u64>,
    chunk_size: usize,
) -> BoxStream<'static, Result<Bytes, super::Error>> {
    futures_util::stream::once(async move {
        let requested = range.end - range.start;

        let (file, path) = maybe_spawn_blocking(move || {
            file.seek(SeekFrom::Start(range.start as _))
                .map_err(|err| map_seek_error(err, &file, &path, range.start))?;
            Ok((file, path))
        })
        .await?;

        let stream = futures_util::stream::try_unfold(
            (file, path, requested),
            move |(mut file, path, remaining)| {
                maybe_spawn_blocking(move || {
                    if remaining == 0 {
                        return Ok(None);
                    }

                    let to_read = remaining.min(chunk_size as u64);
                    let cap = usize::try_from(to_read).map_err(|_e| Error::InvalidRange {
                        source: InvalidGetRange::TooLarge {
                            requested: to_read,
                            max: usize::MAX as u64,
                        },
                    })?;
                    let mut buffer = Vec::with_capacity(cap);
                    let read = (&mut file)
                        .take(to_read)
                        .read_to_end(&mut buffer)
                        .map_err(|e| Error::UnableToReadBytes {
                            source: e,
                            path: path.clone(),
                        })?;

                    Ok(Some((buffer.into(), (file, path, remaining - read as u64))))
                })
            },
        );
        Ok::<_, super::Error>(stream)
    })
    .try_flatten()
    .boxed()
}

pub(crate) fn read_range(
    file: &mut File,
    path: &std::path::Path,
    range: Range<u64>,
) -> Result<Bytes> {
    let requested = range.end - range.start;

    let mut buf = Vec::with_capacity(requested as usize);

    #[cfg(any(target_family = "unix", target_family = "windows"))]
    {
        buf.resize(requested as usize, 0_u8);

        let mut buf_slice = &mut buf[..];
        let mut offset = range.start;

        while !buf_slice.is_empty() {
            #[cfg(target_family = "unix")]
            let read_result = file.read_at(buf_slice, offset);

            #[cfg(target_family = "windows")]
            let read_result = file.seek_read(buf_slice, offset);

            match read_result {
                Ok(0) => break,
                Ok(n) => {
                    let tmp = buf_slice;
                    buf_slice = &mut tmp[n..];
                    offset += n as u64;
                }
                // This error is recoverable
                Err(e) if e.kind() == ErrorKind::Interrupted => {}
                Err(source) => {
                    let error = Error::UnableToReadBytes {
                        source,
                        path: path.into(),
                    };

                    return Err(error.into());
                }
            }
        }

        // If we reached EOF before filling the buffer
        if !buf_slice.is_empty() {
            let metadata = open_metadata(file, path)?;
            let file_len = metadata.len();

            // If none of the range is satisfiable we should error, e.g. if the start offset is beyond the
            // extents of the file, or if its at the end of the file and wants to read a non-empty range.
            // if range.start > file_len || (range.start == file_len && !range.is_empty()) {
            if range.start >= file_len {
                return Err(Error::InvalidRange {
                    source: InvalidGetRange::StartTooLarge {
                        requested: range.start,
                        length: file_len,
                    },
                }
                .into());
            }

            let expected = range.end.min(file_len) - range.start;

            let error = Error::OutOfRange {
                path: path.into(),
                expected,
                actual: offset - range.start,
            };

            return Err(error.into());
        }
    }
    #[cfg(all(not(windows), not(unix)))]
    {
        file.seek(SeekFrom::Start(range.start))
            .map_err(|err| map_seek_error(err, file, path, range.start))?;

        let read = file.take(requested).read_to_end(&mut buf).map_err(|err| {
            // try to read metadata to give a better error in case of directory
            if let Err(e) = open_metadata(file, path) {
                return e;
            }
            Error::UnableToReadBytes {
                source: err,
                path: path.to_path_buf(),
            }
        })? as u64;

        if read != requested {
            let metadata = open_metadata(file, path)?;
            let file_len = metadata.len();

            if range.start >= file_len {
                return Err(Error::InvalidRange {
                    source: InvalidGetRange::StartTooLarge {
                        requested: range.start,
                        length: file_len,
                    },
                }
                .into());
            }

            let expected = range.end.min(file_len) - range.start;
            if read != expected {
                return Err(Error::OutOfRange {
                    path: path.to_path_buf(),
                    expected,
                    actual: read,
                }
                .into());
            }
        }
    }

    Ok(buf.into())
}

fn open_file(path: &std::path::Path) -> Result<File, Error> {
    File::open(path).map_err(|e| map_open_error(e, path))
}

fn open_metadata(file: &File, path: &std::path::Path) -> Result<Metadata, Error> {
    let metadata = file.metadata().map_err(|e| map_open_error(e, path))?;
    if metadata.is_dir() {
        Err(Error::NotFound {
            path: PathBuf::from(path),
            source: io::Error::new(ErrorKind::NotFound, "is directory"),
        })
    } else {
        Ok(metadata)
    }
}

/// Translates errors from opening a file into a more specific [`Error`] when possible
fn map_open_error(source: io::Error, path: &std::path::Path) -> Error {
    let path = PathBuf::from(path);
    match source.kind() {
        ErrorKind::NotFound => Error::NotFound { path, source },
        _ => Error::UnableToOpenFile { path, source },
    }
}

/// Translates errors from attempting to a file into a more specific [`Error`] when possible
fn map_seek_error(source: io::Error, file: &File, path: &std::path::Path, requested: u64) -> Error {
    // if we can't seek, check if start is out of bounds to give
    // a better error. Don't read metadata before to avoid
    // an extra syscall in the common case
    let m = match open_metadata(file, path) {
        Err(e) => return e,
        Ok(m) => m,
    };
    if requested >= m.len() {
        return Error::InvalidRange {
            source: InvalidGetRange::StartTooLarge {
                requested,
                length: m.len(),
            },
        };
    }
    Error::Seek {
        source,
        path: PathBuf::from(path),
    }
}

fn convert_entry(entry: DirEntry, location: Path) -> Result<Option<ObjectMeta>> {
    match entry.metadata() {
        Ok(metadata) => Ok(Some(convert_metadata(metadata, location))),
        Err(e) => {
            if let Some(io_err) = e.io_error() {
                if io_err.kind() == ErrorKind::NotFound {
                    return Ok(None);
                }
            }
            Err(Error::Metadata {
                source: e.into(),
                path: location.to_string(),
            })?
        }
    }
}

fn last_modified(metadata: &Metadata) -> DateTime<Utc> {
    metadata
        .modified()
        .expect("Modified file time should be supported on this platform")
        .into()
}

fn get_etag(metadata: &Metadata) -> String {
    let inode = get_inode(metadata);
    let size = metadata.len();
    let mtime = metadata
        .modified()
        .ok()
        .and_then(|mtime| mtime.duration_since(SystemTime::UNIX_EPOCH).ok())
        .unwrap_or_default()
        .as_micros();

    // Use an ETag scheme based on that used by many popular HTTP servers
    // <https://httpd.apache.org/docs/2.2/mod/core.html#fileetag>
    // <https://stackoverflow.com/questions/47512043/how-etags-are-generated-and-configured>
    format!("{inode:x}-{mtime:x}-{size:x}")
}

fn convert_metadata(metadata: Metadata, location: Path) -> ObjectMeta {
    let last_modified = last_modified(&metadata);

    ObjectMeta {
        location,
        last_modified,
        size: metadata.len(),
        e_tag: Some(get_etag(&metadata)),
        version: None,
    }
}

#[cfg(unix)]
/// We include the inode when available to yield an ETag more resistant to collisions
/// and as used by popular web servers such as [Apache](https://httpd.apache.org/docs/2.2/mod/core.html#fileetag)
fn get_inode(metadata: &Metadata) -> u64 {
    std::os::unix::fs::MetadataExt::ino(metadata)
}

#[cfg(not(unix))]
/// On platforms where an inode isn't available, fallback to just relying on size and mtime
fn get_inode(_metadata: &Metadata) -> u64 {
    0
}

/// Convert walkdir results and converts not-found errors into `None`.
/// Convert broken symlinks to `None`.
fn convert_walkdir_result(
    res: std::result::Result<DirEntry, walkdir::Error>,
) -> Result<Option<DirEntry>> {
    match res {
        Ok(entry) => {
            // To check for broken symlink: call symlink_metadata() - it does not traverse symlinks);
            // if ok: check if entry is symlink; and try to read it by calling metadata().
            match symlink_metadata(entry.path()) {
                Ok(attr) => {
                    if attr.is_symlink() {
                        let target_metadata = metadata(entry.path());
                        match target_metadata {
                            Ok(_) => {
                                // symlink is valid
                                Ok(Some(entry))
                            }
                            Err(_) => {
                                // this is a broken symlink, return None
                                Ok(None)
                            }
                        }
                    } else {
                        Ok(Some(entry))
                    }
                }
                Err(_) => Ok(None),
            }
        }

        Err(walkdir_err) => match walkdir_err.io_error() {
            Some(io_err) => match io_err.kind() {
                ErrorKind::NotFound => Ok(None),
                _ => Err(Error::UnableToWalkDir {
                    source: walkdir_err,
                }
                .into()),
            },
            None => Err(Error::UnableToWalkDir {
                source: walkdir_err,
            }
            .into()),
        },
    }
}

#[cfg(test)]
mod tests {
    use std::fs;

    use futures_util::TryStreamExt;
    use tempfile::TempDir;

    #[cfg(target_family = "unix")]
    use tempfile::NamedTempFile;

    use crate::{integration::*, ObjectStoreExt};

    use super::*;

    #[tokio::test]
    #[cfg(target_family = "unix")]
    async fn file_test() {
        let root = TempDir::new().unwrap();
        let integration = LocalFileSystem::new_with_prefix(root.path()).unwrap();

        put_get_delete_list(&integration).await;
        list_with_offset_exclusivity(&integration).await;
        get_opts(&integration).await;
        list_uses_directories_correctly(&integration).await;
        list_with_delimiter(&integration).await;
        rename_and_copy(&integration).await;
        copy_if_not_exists(&integration).await;
        copy_rename_nonexistent_object(&integration).await;
        stream_get(&integration).await;
        put_opts(&integration, false).await;
    }

    #[test]
    #[cfg(target_family = "unix")]
    fn test_non_tokio() {
        let root = TempDir::new().unwrap();
        let integration = LocalFileSystem::new_with_prefix(root.path()).unwrap();
        futures_executor::block_on(async move {
            put_get_delete_list(&integration).await;
            list_uses_directories_correctly(&integration).await;
            list_with_delimiter(&integration).await;

            // Can't use stream_get test as WriteMultipart uses a tokio JoinSet
            let p = Path::from("manual_upload");
            let mut upload = integration.put_multipart(&p).await.unwrap();
            upload.put_part("123".into()).await.unwrap();
            upload.put_part("45678".into()).await.unwrap();
            let r = upload.complete().await.unwrap();

            let get = integration.get(&p).await.unwrap();
            assert_eq!(get.meta.e_tag.as_ref().unwrap(), r.e_tag.as_ref().unwrap());
            let actual = get.bytes().await.unwrap();
            assert_eq!(actual.as_ref(), b"12345678");
        });
    }

    #[tokio::test]
    async fn creates_dir_if_not_present() {
        let root = TempDir::new().unwrap();
        let integration = LocalFileSystem::new_with_prefix(root.path()).unwrap();

        let location = Path::from("nested/file/test_file");

        let data = Bytes::from("arbitrary data");

        integration
            .put(&location, data.clone().into())
            .await
            .unwrap();

        let read_data = integration
            .get(&location)
            .await
            .unwrap()
            .bytes()
            .await
            .unwrap();
        assert_eq!(&*read_data, data);
    }

    #[tokio::test]
    async fn unknown_length() {
        let root = TempDir::new().unwrap();
        let integration = LocalFileSystem::new_with_prefix(root.path()).unwrap();

        let location = Path::from("some_file");

        let data = Bytes::from("arbitrary data");

        integration
            .put(&location, data.clone().into())
            .await
            .unwrap();

        let read_data = integration
            .get(&location)
            .await
            .unwrap()
            .bytes()
            .await
            .unwrap();
        assert_eq!(&*read_data, data);
    }

    #[tokio::test]
    async fn range_request_start_beyond_end_of_file() {
        let root = TempDir::new().unwrap();
        let integration = LocalFileSystem::new_with_prefix(root.path()).unwrap();

        let location = Path::from("some_file");

        let data = Bytes::from("arbitrary data");

        integration
            .put(&location, data.clone().into())
            .await
            .unwrap();

        integration
            .get_range(&location, 100..200)
            .await
            .expect_err("Should error with start range beyond end of file");
    }

    #[tokio::test]
    async fn range_request_beyond_end_of_file() {
        let root = TempDir::new().unwrap();
        let integration = LocalFileSystem::new_with_prefix(root.path()).unwrap();

        let location = Path::from("some_file");

        let data = Bytes::from("arbitrary data");

        integration
            .put(&location, data.clone().into())
            .await
            .unwrap();

        let read_data = integration.get_range(&location, 0..100).await.unwrap();
        assert_eq!(&*read_data, data);
    }

    #[tokio::test]
    #[cfg(target_family = "unix")]
    // Fails on github actions runner (which runs the tests as root)
    #[ignore]
    async fn bubble_up_io_errors() {
        use std::{fs::set_permissions, os::unix::prelude::PermissionsExt};

        let root = TempDir::new().unwrap();

        // make non-readable
        let metadata = root.path().metadata().unwrap();
        let mut permissions = metadata.permissions();
        permissions.set_mode(0o000);
        set_permissions(root.path(), permissions).unwrap();

        let store = LocalFileSystem::new_with_prefix(root.path()).unwrap();

        let mut stream = store.list(None);
        let mut any_err = false;
        while let Some(res) = stream.next().await {
            if res.is_err() {
                any_err = true;
            }
        }
        assert!(any_err);

        // `list_with_delimiter
        assert!(store.list_with_delimiter(None).await.is_err());
    }

    const NON_EXISTENT_NAME: &str = "nonexistentname";

    #[tokio::test]
    async fn get_nonexistent_location() {
        let root = TempDir::new().unwrap();
        let integration = LocalFileSystem::new_with_prefix(root.path()).unwrap();

        let location = Path::from(NON_EXISTENT_NAME);

        let err = get_nonexistent_object(&integration, Some(location))
            .await
            .unwrap_err();
        if let crate::Error::NotFound { path, source } = err {
            let source_variant = source.downcast_ref::<std::io::Error>();
            assert!(
                matches!(source_variant, Some(std::io::Error { .. }),),
                "got: {source_variant:?}"
            );
            assert!(path.ends_with(NON_EXISTENT_NAME), "{}", path);
        } else {
            panic!("unexpected error type: {err:?}");
        }
    }

    #[tokio::test]
    async fn root() {
        let integration = LocalFileSystem::new();

        let canonical = std::path::Path::new("Cargo.toml").canonicalize().unwrap();
        let url = Url::from_directory_path(&canonical).unwrap();
        let path = Path::parse(url.path()).unwrap();

        let roundtrip = integration.path_to_filesystem(&path).unwrap();

        // Needed as on Windows canonicalize returns extended length path syntax
        // C:\Users\circleci -> \\?\C:\Users\circleci
        let roundtrip = roundtrip.canonicalize().unwrap();

        assert_eq!(roundtrip, canonical);

        integration.head(&path).await.unwrap();
    }

    #[tokio::test]
    #[cfg(target_family = "windows")]
    async fn test_list_root() {
        let fs = LocalFileSystem::new();
        let r = fs.list_with_delimiter(None).await.unwrap_err().to_string();

        assert!(
            r.contains("Unable to convert URL \"file:///\" to filesystem path"),
            "{}",
            r
        );
    }

    #[tokio::test]
    #[cfg(target_os = "linux")]
    async fn test_list_root() {
        let fs = LocalFileSystem::new();
        fs.list_with_delimiter(None).await.unwrap();
    }

    #[cfg(target_family = "unix")]
    async fn check_list(integration: &LocalFileSystem, prefix: Option<&Path>, expected: &[&str]) {
        let result: Vec<_> = integration.list(prefix).try_collect().await.unwrap();

        let mut strings: Vec<_> = result.iter().map(|x| x.location.as_ref()).collect();
        strings.sort_unstable();
        assert_eq!(&strings, expected)
    }

    #[tokio::test]
    #[cfg(target_family = "unix")]
    async fn test_symlink() {
        let root = TempDir::new().unwrap();
        let integration = LocalFileSystem::new_with_prefix(root.path()).unwrap();

        let subdir = root.path().join("a");
        std::fs::create_dir(&subdir).unwrap();
        let file = subdir.join("file.parquet");
        std::fs::write(file, "test").unwrap();

        check_list(&integration, None, &["a/file.parquet"]).await;
        integration
            .head(&Path::from("a/file.parquet"))
            .await
            .unwrap();

        // Follow out of tree symlink
        let other = NamedTempFile::new().unwrap();
        std::os::unix::fs::symlink(other.path(), root.path().join("test.parquet")).unwrap();

        // Should return test.parquet even though out of tree
        check_list(&integration, None, &["a/file.parquet", "test.parquet"]).await;

        // Can fetch test.parquet
        integration.head(&Path::from("test.parquet")).await.unwrap();

        // Follow in tree symlink
        std::os::unix::fs::symlink(&subdir, root.path().join("b")).unwrap();
        check_list(
            &integration,
            None,
            &["a/file.parquet", "b/file.parquet", "test.parquet"],
        )
        .await;
        check_list(&integration, Some(&Path::from("b")), &["b/file.parquet"]).await;

        // Can fetch through symlink
        integration
            .head(&Path::from("b/file.parquet"))
            .await
            .unwrap();

        // Ignore broken symlink
        std::os::unix::fs::symlink(root.path().join("foo.parquet"), root.path().join("c")).unwrap();

        check_list(
            &integration,
            None,
            &["a/file.parquet", "b/file.parquet", "test.parquet"],
        )
        .await;

        let mut r = integration.list_with_delimiter(None).await.unwrap();
        r.common_prefixes.sort_unstable();
        assert_eq!(r.common_prefixes.len(), 2);
        assert_eq!(r.common_prefixes[0].as_ref(), "a");
        assert_eq!(r.common_prefixes[1].as_ref(), "b");
        assert_eq!(r.objects.len(), 1);
        assert_eq!(r.objects[0].location.as_ref(), "test.parquet");

        let r = integration
            .list_with_delimiter(Some(&Path::from("a")))
            .await
            .unwrap();
        assert_eq!(r.common_prefixes.len(), 0);
        assert_eq!(r.objects.len(), 1);
        assert_eq!(r.objects[0].location.as_ref(), "a/file.parquet");

        // Deleting a symlink doesn't delete the source file
        integration
            .delete(&Path::from("test.parquet"))
            .await
            .unwrap();
        assert!(other.path().exists());

        check_list(&integration, None, &["a/file.parquet", "b/file.parquet"]).await;

        // Deleting through a symlink deletes both files
        integration
            .delete(&Path::from("b/file.parquet"))
            .await
            .unwrap();

        check_list(&integration, None, &[]).await;

        // Adding a file through a symlink creates in both paths
        integration
            .put(&Path::from("b/file.parquet"), vec![0, 1, 2].into())
            .await
            .unwrap();

        check_list(&integration, None, &["a/file.parquet", "b/file.parquet"]).await;
    }

    #[tokio::test]
    async fn invalid_path() {
        let root = TempDir::new().unwrap();
        let root = root.path().join("🙀");
        std::fs::create_dir(root.clone()).unwrap();

        // Invalid paths supported above root of store
        let integration = LocalFileSystem::new_with_prefix(root.clone()).unwrap();

        let directory = Path::from("directory");
        let object = directory.clone().join("child.txt");
        let data = Bytes::from("arbitrary");
        integration.put(&object, data.clone().into()).await.unwrap();
        integration.head(&object).await.unwrap();
        let result = integration.get(&object).await.unwrap();
        assert_eq!(result.bytes().await.unwrap(), data);

        flatten_list_stream(&integration, None).await.unwrap();
        flatten_list_stream(&integration, Some(&directory))
            .await
            .unwrap();

        let result = integration
            .list_with_delimiter(Some(&directory))
            .await
            .unwrap();
        assert_eq!(result.objects.len(), 1);
        assert!(result.common_prefixes.is_empty());
        assert_eq!(result.objects[0].location, object);

        let emoji = root.join("💀");
        std::fs::write(emoji, "foo").unwrap();

        // Can list illegal file
        let mut paths = flatten_list_stream(&integration, None).await.unwrap();
        paths.sort_unstable();

        assert_eq!(
            paths,
            vec![
                Path::parse("directory/child.txt").unwrap(),
                Path::parse("💀").unwrap()
            ]
        );
    }

    #[tokio::test]
    async fn list_hides_incomplete_uploads() {
        let root = TempDir::new().unwrap();
        let integration = LocalFileSystem::new_with_prefix(root.path()).unwrap();
        let location = Path::from("some_file");

        let data = PutPayload::from("arbitrary data");
        let mut u1 = integration.put_multipart(&location).await.unwrap();
        u1.put_part(data.clone()).await.unwrap();

        let mut u2 = integration.put_multipart(&location).await.unwrap();
        u2.put_part(data).await.unwrap();

        let list = flatten_list_stream(&integration, None).await.unwrap();
        assert_eq!(list.len(), 0);

        assert_eq!(
            integration
                .list_with_delimiter(None)
                .await
                .unwrap()
                .objects
                .len(),
            0
        );
    }

    #[tokio::test]
    async fn test_path_with_offset() {
        let root = TempDir::new().unwrap();
        let integration = LocalFileSystem::new_with_prefix(root.path()).unwrap();

        let root_path = root.path();
        for i in 0..5 {
            let filename = format!("test{i}.parquet");
            let file = root_path.join(filename);
            std::fs::write(file, "test").unwrap();
        }
        let filter_str = "test";
        let filter = String::from(filter_str);
        let offset_str = filter + "1";
        let offset = Path::from(offset_str.clone());

        // Use list_with_offset to retrieve files
        let res = integration.list_with_offset(None, &offset);
        let offset_paths: Vec<_> = res.map_ok(|x| x.location).try_collect().await.unwrap();
        let mut offset_files: Vec<_> = offset_paths
            .iter()
            .map(|x| String::from(x.filename().unwrap()))
            .collect();

        // Check result with direct filesystem read
        let files = fs::read_dir(root_path).unwrap();
        let filtered_files = files
            .filter_map(Result::ok)
            .filter_map(|d| {
                d.file_name().to_str().and_then(|f| {
                    if f.contains(filter_str) {
                        Some(String::from(f))
                    } else {
                        None
                    }
                })
            })
            .collect::<Vec<_>>();

        let mut expected_offset_files: Vec<_> = filtered_files
            .iter()
            .filter(|s| **s > offset_str)
            .cloned()
            .collect();

        fn do_vecs_match<T: PartialEq>(a: &[T], b: &[T]) -> bool {
            let matching = a.iter().zip(b.iter()).filter(|&(a, b)| a == b).count();
            matching == a.len() && matching == b.len()
        }

        offset_files.sort();
        expected_offset_files.sort();

        // println!("Expected Offset Files: {:?}", expected_offset_files);
        // println!("Actual Offset Files: {:?}", offset_files);

        assert_eq!(offset_files.len(), expected_offset_files.len());
        assert!(do_vecs_match(&expected_offset_files, &offset_files));
    }

    #[tokio::test]
    async fn filesystem_filename_with_percent() {
        let temp_dir = TempDir::new().unwrap();
        let integration = LocalFileSystem::new_with_prefix(temp_dir.path()).unwrap();
        let filename = "L%3ABC.parquet";

        std::fs::write(temp_dir.path().join(filename), "foo").unwrap();

        let res: Vec<_> = integration.list(None).try_collect().await.unwrap();
        assert_eq!(res.len(), 1);
        assert_eq!(res[0].location.as_ref(), filename);

        let res = integration.list_with_delimiter(None).await.unwrap();
        assert_eq!(res.objects.len(), 1);
        assert_eq!(res.objects[0].location.as_ref(), filename);
    }

    #[tokio::test]
    async fn relative_paths() {
        LocalFileSystem::new_with_prefix(".").unwrap();
        LocalFileSystem::new_with_prefix("..").unwrap();
        LocalFileSystem::new_with_prefix("../..").unwrap();

        let integration = LocalFileSystem::new();
        let path = Path::from_filesystem_path(".").unwrap();
        integration.list_with_delimiter(Some(&path)).await.unwrap();
    }

    #[test]
    fn test_valid_path() {
        let cases = [
            ("foo#123/test.txt", true),
            ("foo#123/test#23.txt", true),
            ("foo#123/test#34", false),
            ("foo😁/test#34", false),
            ("foo/test#😁34", true),
        ];

        for (case, expected) in cases {
            let path = Path::parse(case).unwrap();
            assert_eq!(is_valid_file_path(&path), expected);
        }
    }

    #[tokio::test]
    async fn test_intermediate_files() {
        let root = TempDir::new().unwrap();
        let integration = LocalFileSystem::new_with_prefix(root.path()).unwrap();

        let a = Path::parse("foo#123/test.txt").unwrap();
        integration.put(&a, "test".into()).await.unwrap();

        let list = flatten_list_stream(&integration, None).await.unwrap();
        assert_eq!(list, vec![a.clone()]);

        std::fs::write(root.path().join("bar#123"), "test").unwrap();

        // Should ignore file
        let list = flatten_list_stream(&integration, None).await.unwrap();
        assert_eq!(list, vec![a.clone()]);

        let b = Path::parse("bar#123").unwrap();
        let err = integration.get(&b).await.unwrap_err().to_string();
        assert_eq!(
            err,
            "Generic LocalFileSystem error: Filenames containing trailing '/#\\d+/' are not supported: bar#123"
        );

        let c = Path::parse("foo#123.txt").unwrap();
        integration.put(&c, "test".into()).await.unwrap();

        let mut list = flatten_list_stream(&integration, None).await.unwrap();
        list.sort_unstable();
        assert_eq!(list, vec![c, a]);
    }

    #[tokio::test]
    #[cfg(target_os = "windows")]
    async fn filesystem_filename_with_colon() {
        let root = TempDir::new().unwrap();
        let integration = LocalFileSystem::new_with_prefix(root.path()).unwrap();
        let path = Path::parse("file%3Aname.parquet").unwrap();
        let location = Path::parse("file:name.parquet").unwrap();

        integration.put(&location, "test".into()).await.unwrap();
        let list = flatten_list_stream(&integration, None).await.unwrap();
        assert_eq!(list, vec![path.clone()]);

        let result = integration
            .get(&location)
            .await
            .unwrap()
            .bytes()
            .await
            .unwrap();
        assert_eq!(result, Bytes::from("test"));
    }

    #[tokio::test]
    async fn delete_dirs_automatically() {
        let root = TempDir::new().unwrap();
        let integration = LocalFileSystem::new_with_prefix(root.path())
            .unwrap()
            .with_automatic_cleanup(true);
        let location = Path::from("nested/file/test_file");
        let data = Bytes::from("arbitrary data");

        integration
            .put(&location, data.clone().into())
            .await
            .unwrap();

        let read_data = integration
            .get(&location)
            .await
            .unwrap()
            .bytes()
            .await
            .unwrap();

        assert_eq!(&*read_data, data);
        assert!(fs::read_dir(root.path()).unwrap().count() > 0);
        integration.delete(&location).await.unwrap();
        assert!(fs::read_dir(root.path()).unwrap().count() == 0);
    }
}

#[cfg(not(target_arch = "wasm32"))]
#[cfg(test)]
mod not_wasm_tests {
    use std::time::Duration;
    use tempfile::TempDir;

    use crate::local::LocalFileSystem;
    use crate::{ObjectStoreExt, Path, PutPayload};

    #[tokio::test]
    async fn test_cleanup_intermediate_files() {
        let root = TempDir::new().unwrap();
        let integration = LocalFileSystem::new_with_prefix(root.path()).unwrap();

        let location = Path::from("some_file");
        let data = PutPayload::from_static(b"hello");
        let mut upload = integration.put_multipart(&location).await.unwrap();
        upload.put_part(data).await.unwrap();

        let file_count = std::fs::read_dir(root.path()).unwrap().count();
        assert_eq!(file_count, 1);
        drop(upload);

        for _ in 0..100 {
            tokio::time::sleep(Duration::from_millis(1)).await;
            let file_count = std::fs::read_dir(root.path()).unwrap().count();
            if file_count == 0 {
                return;
            }
        }
        panic!("Failed to cleanup file in 100ms")
    }
}

#[cfg(target_family = "unix")]
#[cfg(test)]
mod unix_test {
    use std::fs::OpenOptions;

    use nix::sys::stat;
    use nix::unistd;
    use tempfile::TempDir;

    use crate::local::LocalFileSystem;
    use crate::{ObjectStoreExt, Path};

    #[tokio::test]
    async fn test_fifo() {
        let filename = "some_file";
        let root = TempDir::new().unwrap();
        let integration = LocalFileSystem::new_with_prefix(root.path()).unwrap();
        let path = root.path().join(filename);
        unistd::mkfifo(&path, stat::Mode::S_IRWXU).unwrap();

        // Need to open read and write side in parallel
        let spawned =
            tokio::task::spawn_blocking(|| OpenOptions::new().write(true).open(path).unwrap());

        let location = Path::from(filename);
        integration.head(&location).await.unwrap();
        integration.get(&location).await.unwrap();

        spawned.await.unwrap();
    }
}
