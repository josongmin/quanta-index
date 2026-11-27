//! ObjectStore upload scheduling for the product-owned staging writer.

use std::fmt;
use std::future;
use std::io;
use std::path::PathBuf;

use async_trait::async_trait;
use bytes::Bytes;
use object_store::{MultipartUpload, PutPayload, PutResult, Result as StoreResult, UploadPart};

use crate::SemanticStagingUpload;

use super::{blocking_file_error, core_file_error, file_error, local_put_result};

pub(super) struct StagingUpload {
    path: PathBuf,
    upload: Option<Box<dyn SemanticStagingUpload>>,
}

impl StagingUpload {
    pub(super) fn new(path: PathBuf, upload: Box<dyn SemanticStagingUpload>) -> Self {
        Self {
            path,
            upload: Some(upload),
        }
    }

    fn closed(&self) -> object_store::Error {
        file_error(&self.path, io::Error::other("staging upload is closed"))
    }
}

impl fmt::Debug for StagingUpload {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("StagingUpload")
            .field("path", &self.path)
            .finish_non_exhaustive()
    }
}

#[async_trait]
impl MultipartUpload for StagingUpload {
    fn put_part(&mut self, data: PutPayload) -> UploadPart {
        let part = (|| {
            let bytes = data.iter().try_fold(0_u64, |total, chunk| {
                u64::try_from(chunk.len())
                    .ok()
                    .and_then(|length| total.checked_add(length))
                    .ok_or_else(|| {
                        file_error(&self.path, io::Error::other("multipart length overflow"))
                    })
            })?;
            self.upload
                .as_mut()
                .ok_or_else(|| {
                    file_error(&self.path, io::Error::other("staging upload is closed"))
                })?
                .reserve_part(bytes)
                .map_err(|error| core_file_error(&self.path, error))
        })();
        let part = match part {
            Ok(part) => part,
            Err(error) => return Box::pin(future::ready(Err(error))),
        };
        let path = self.path.clone();
        Box::pin(async move {
            tokio::task::spawn_blocking(move || {
                let chunks: Vec<_> = data.iter().map(Bytes::as_ref).collect();
                part.write(&chunks)
                    .map_err(|error| core_file_error(&path, error))
            })
            .await
            .map_err(blocking_file_error)?
        })
    }

    async fn complete(&mut self) -> StoreResult<PutResult> {
        let upload = self.upload.take().ok_or_else(|| self.closed())?;
        let path = self.path.clone();
        tokio::task::spawn_blocking(move || {
            let metadata = upload
                .complete()
                .map_err(|error| core_file_error(&path, error))?;
            Ok(local_put_result(&metadata))
        })
        .await
        .map_err(blocking_file_error)?
    }

    async fn abort(&mut self) -> StoreResult<()> {
        let upload = self.upload.take().ok_or_else(|| self.closed())?;
        let path = self.path.clone();
        tokio::task::spawn_blocking(move || {
            upload
                .abort()
                .map_err(|error| core_file_error(&path, error))
        })
        .await
        .map_err(blocking_file_error)?
    }
}

impl Drop for StagingUpload {
    fn drop(&mut self) {
        let Some(upload) = self.upload.take() else {
            return;
        };
        // The product upload owns checked retirement even when a queued
        // blocker is discarded during runtime shutdown.
        if let Ok(runtime) = tokio::runtime::Handle::try_current() {
            drop(runtime.spawn_blocking(move || {
                let _cleanup = upload.abort();
            }));
        } else {
            let _cleanup = upload.abort();
        }
    }
}
