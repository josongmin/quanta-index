// SPDX-License-Identifier: Apache-2.0
// SPDX-FileCopyrightText: Copyright The Lance Authors

//! Mutations that Lance sends directly to `ObjectStore.inner` on a local URI.

use std::fmt;
use std::io;
use std::sync::Arc;

use async_trait::async_trait;
use futures::{
    stream::{self, BoxStream},
    StreamExt,
};
use object_store::local::LocalFileSystem;
use object_store::path::Path;
use object_store::{
    CopyMode, CopyOptions, GetOptions, GetResult, ListResult, MultipartUpload, ObjectMeta,
    ObjectStore, PutMultipartOptions, PutOptions, PutPayload, PutResult, RenameOptions,
    RenameTargetMode, Result as OSResult,
};

use super::file_admission::{FileAdmission, FileMoveMode, FilePathTicket};
use crate::local::to_local_path;

fn admission_error(error: io::Error) -> object_store::Error {
    object_store::Error::Generic {
        store: "admitted-file",
        source: Box::new(error),
    }
}

#[derive(Debug)]
struct AdmittedCleanupFailure {
    primary: object_store::Error,
    cleanup: object_store::Error,
}

impl fmt::Display for AdmittedCleanupFailure {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "admitted operation failed: {}; staging cleanup failed: {}",
            self.primary, self.cleanup
        )
    }
}

impl std::error::Error for AdmittedCleanupFailure {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(&self.primary)
    }
}

fn local_path(path: &Path) -> std::path::PathBuf {
    std::path::PathBuf::from(to_local_path(path))
}

fn local_mutation_error(path: &std::path::Path, error: io::Error) -> object_store::Error {
    let path = path.to_string_lossy().into_owned();
    match error.kind() {
        io::ErrorKind::AlreadyExists => object_store::Error::AlreadyExists {
            path,
            source: Box::new(error),
        },
        io::ErrorKind::NotFound => object_store::Error::NotFound {
            path,
            source: Box::new(error),
        },
        _ => admission_error(error),
    }
}

fn namespace_mutation_error(
    from: &std::path::Path,
    to: &std::path::Path,
    error: io::Error,
) -> object_store::Error {
    if error.kind() == io::ErrorKind::NotFound {
        local_mutation_error(from, error)
    } else {
        local_mutation_error(to, error)
    }
}

fn admitted_link(
    admission: &dyn FileAdmission,
    from: &std::path::Path,
    to: &std::path::Path,
) -> OSResult<()> {
    let mut ticket = admission
        .begin_link(from, to)
        .map_err(|error| namespace_mutation_error(from, to, error))?;
    ticket
        .perform_and_sync()
        .map_err(|error| namespace_mutation_error(from, to, error))
}

fn admitted_move(
    admission: &dyn FileAdmission,
    from: &std::path::Path,
    to: &std::path::Path,
    mode: FileMoveMode,
) -> OSResult<()> {
    let ticket = admission
        .begin_move(from, to, mode)
        .map_err(|error| namespace_mutation_error(from, to, error))?;
    admitted_move_with_ticket(ticket, from, to)
}

fn admitted_move_with_ticket(
    mut ticket: Box<dyn FilePathTicket>,
    from: &std::path::Path,
    to: &std::path::Path,
) -> OSResult<()> {
    ticket
        .perform_and_sync()
        .map_err(|error| namespace_mutation_error(from, to, error))
}

fn admitted_delete(admission: &dyn FileAdmission, path: &std::path::Path) -> OSResult<()> {
    admission
        .delete_file_and_sync(path)
        .map_err(|error| local_mutation_error(path, error))
}

#[derive(Debug)]
pub(super) struct AdmittedInnerStore {
    target: Arc<LocalFileSystem>,
    admission: Arc<dyn FileAdmission>,
}

impl AdmittedInnerStore {
    pub(super) fn new(target: Arc<LocalFileSystem>, admission: Arc<dyn FileAdmission>) -> Self {
        Self { target, admission }
    }
}

impl fmt::Display for AdmittedInnerStore {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "AdmittedInnerStore({})", self.target)
    }
}

#[async_trait]
impl ObjectStore for AdmittedInnerStore {
    async fn put_opts(
        &self,
        location: &Path,
        payload: PutPayload,
        opts: PutOptions,
    ) -> OSResult<PutResult> {
        // The admitted LocalFileSystem owns the exact private staging FD and
        // ticket inside its blocking task. Wrapping it again would double book.
        self.target.put_opts(location, payload, opts).await
    }

    async fn put_multipart_opts(
        &self,
        location: &Path,
        opts: PutMultipartOptions,
    ) -> OSResult<Box<dyn MultipartUpload>> {
        self.target.put_multipart_opts(location, opts).await
    }

    async fn get_opts(&self, location: &Path, options: GetOptions) -> OSResult<GetResult> {
        self.admission
            .reconcile_pending_namespace()
            .map_err(admission_error)?;
        self.target.get_opts(location, options).await
    }

    fn delete_stream(
        &self,
        locations: BoxStream<'static, OSResult<Path>>,
    ) -> BoxStream<'static, OSResult<Path>> {
        let admission = Arc::clone(&self.admission);
        locations
            .then(move |item| {
                let admission = Arc::clone(&admission);
                async move {
                    let path = item?;
                    let local = local_path(&path);
                    tokio::task::spawn_blocking(move || {
                        admitted_delete(admission.as_ref(), &local)
                    })
                    .await
                    .map_err(|error| admission_error(io::Error::other(error)))??;
                    Ok(path)
                }
            })
            .boxed()
    }

    fn list(&self, prefix: Option<&Path>) -> BoxStream<'static, OSResult<ObjectMeta>> {
        if let Err(error) = self.admission.reconcile_pending_namespace() {
            return stream::once(async move { Err(admission_error(error)) }).boxed();
        }
        self.target.list(prefix)
    }

    fn list_with_offset(
        &self,
        prefix: Option<&Path>,
        offset: &Path,
    ) -> BoxStream<'static, OSResult<ObjectMeta>> {
        if let Err(error) = self.admission.reconcile_pending_namespace() {
            return stream::once(async move { Err(admission_error(error)) }).boxed();
        }
        self.target.list_with_offset(prefix, offset)
    }

    async fn list_with_delimiter(&self, prefix: Option<&Path>) -> OSResult<ListResult> {
        self.admission
            .reconcile_pending_namespace()
            .map_err(admission_error)?;
        self.target.list_with_delimiter(prefix).await
    }

    async fn copy_opts(&self, from: &Path, to: &Path, opts: CopyOptions) -> OSResult<()> {
        let from_local = local_path(from);
        let to_local = local_path(to);
        let admission = Arc::clone(&self.admission);
        // The task owns every path ticket through syscall and parent sync.
        // In particular, the overwrite staging alias cannot be hidden inside
        // LocalFileSystem::copy_opts, which would escape the binder's inventory.
        tokio::task::spawn_blocking(move || {
            admission
                .reconcile_pending_namespace()
                .map_err(admission_error)?;
            if from_local == to_local {
                return Err(admission_error(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "admitted copy source equals target",
                )));
            }
            admission
                .prepare_parent_dirs(&to_local)
                .map_err(admission_error)?;
            match opts.mode {
                CopyMode::Create => admitted_link(admission.as_ref(), &from_local, &to_local),
                CopyMode::Overwrite => {
                    let parent = to_local.parent().ok_or_else(|| {
                        admission_error(io::Error::other("admitted copy target has no parent"))
                    })?;
                    let staged = parent.join(format!(
                        ".lance-admitted-copy-{:032x}.tmp",
                        rand::random::<u128>()
                    ));
                    admitted_link(admission.as_ref(), &from_local, &staged)?;
                    // A refused ticket has not started a namespace change.
                    // Remove only that staged alias; later move failures may
                    // have published the destination and remain uncertain.
                    let ticket =
                        match admission.begin_move(
                            &staged,
                            &to_local,
                            FileMoveMode::OverwriteFromAlias,
                        ) {
                            Ok(ticket) => ticket,
                            Err(error) => {
                                let primary = namespace_mutation_error(&staged, &to_local, error);
                                return match admitted_delete(admission.as_ref(), &staged) {
                                    Ok(()) => Err(primary),
                                    Err(cleanup) => Err(admission_error(io::Error::other(
                                        AdmittedCleanupFailure { primary, cleanup },
                                    ))),
                                };
                            }
                        };
                    admitted_move_with_ticket(ticket, &staged, &to_local)
                }
            }
        })
        .await
        .map_err(|error| admission_error(io::Error::other(error)))?
    }

    async fn rename_opts(&self, from: &Path, to: &Path, opts: RenameOptions) -> OSResult<()> {
        let from_local = local_path(from);
        let to_local = local_path(to);
        let admission = Arc::clone(&self.admission);
        tokio::task::spawn_blocking(move || {
            admission
                .prepare_parent_dirs(&to_local)
                .map_err(admission_error)?;
            if from_local == to_local {
                return Err(admission_error(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "admitted rename source equals target",
                )));
            }
            let mode = match opts.target_mode {
                RenameTargetMode::Create => FileMoveMode::Create,
                RenameTargetMode::Overwrite => FileMoveMode::Overwrite,
            };
            admitted_move(admission.as_ref(), &from_local, &to_local, mode)
        })
        .await
        .map_err(|error| admission_error(io::Error::other(error)))?
    }
}
