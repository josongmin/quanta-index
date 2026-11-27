//! One capacity ticket and original writer for a staging multipart upload.

use std::fs::{File, Metadata};
use std::io::{self, Seek as _, SeekFrom, Write as _};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use quanta_index_core::CoreError;
use quanta_index_semantic::{
    SemanticStagingPart, SemanticStagingTargetMode, SemanticStagingUpload,
};

use super::staged_file;
use crate::publication_file_capacity::{FileMutation, append_failure};

const UPLOAD_CONTEXT: &str = "semantic staging multipart";

fn upload_error(message: &str) -> CoreError {
    CoreError::Storage(format!("{UPLOAD_CONTEXT}: {message}"))
}

fn storage_io(action: &str, error: &io::Error) -> CoreError {
    CoreError::storage_io(&format!("{UPLOAD_CONTEXT} {action}"), error)
}

pub(super) struct StagedMultipart {
    mutation: Option<FileMutation>,
    writer: Arc<Mutex<UploadWriter>>,
    target: PathBuf,
    next_offset: u64,
    submitted_parts: u64,
}

struct UploadWriter {
    file: Option<File>,
    completed_parts: u64,
}

impl StagedMultipart {
    pub(super) fn open(mutation: FileMutation, target: PathBuf) -> Result<Self, CoreError> {
        let (mutation, writer) = staged_file::open(mutation)?;
        Ok(Self {
            mutation: Some(mutation),
            writer: Arc::new(Mutex::new(UploadWriter {
                file: Some(writer),
                completed_parts: 0,
            })),
            target,
            next_offset: 0,
            submitted_parts: 0,
        })
    }

    fn cancel(&mut self) -> Result<(), CoreError> {
        let Some(mut mutation) = self.mutation.take() else {
            return Ok(());
        };
        // Recover a poisoned guard for physical retirement. Once taken, the
        // original FD is unavailable to queued parts even during cleanup.
        let writer = self
            .writer
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .file
            .take()
            .ok_or_else(|| upload_error("original writer absent during abort"))?;
        mutation.cancel_declared_temp(writer)
    }
    fn install(&mut self) -> Result<Metadata, CoreError> {
        let mut state = self
            .writer
            .lock()
            .map_err(|_| upload_error("writer lock poisoned"))?;
        if state.completed_parts != self.submitted_parts {
            return Err(upload_error("upload parts did not all complete"));
        }
        let metadata = state
            .file
            .as_ref()
            .ok_or_else(|| upload_error("upload is closed"))?
            .metadata()
            .map_err(|error| storage_io("writer metadata", &error))?;
        if metadata.len() != self.next_offset {
            return Err(upload_error(
                "writer width differs from reserved part ranges",
            ));
        }
        let mutation = self
            .mutation
            .take()
            .ok_or_else(|| upload_error("upload has no mutation ticket"))?;
        let writer = state
            .file
            .take()
            .ok_or_else(|| upload_error("upload is closed"))?;
        drop(state);
        if !staged_file::install(
            mutation,
            writer,
            &self.target,
            SemanticStagingTargetMode::Overwrite,
            UPLOAD_CONTEXT,
        )? {
            return Err(upload_error("upload replacement lacked output"));
        }
        Ok(metadata)
    }
}

impl SemanticStagingUpload for StagedMultipart {
    fn reserve_part(&mut self, bytes: u64) -> Result<Box<dyn SemanticStagingPart>, CoreError> {
        let offset = self.next_offset;
        let end = offset
            .checked_add(bytes)
            .ok_or_else(|| upload_error("part offset overflow"))?;
        let submitted = self
            .submitted_parts
            .checked_add(1)
            .ok_or_else(|| upload_error("part count overflow"))?;
        self.mutation
            .as_mut()
            .ok_or_else(|| upload_error("upload is closed"))?
            .reserve_end(end)?;
        self.next_offset = end;
        self.submitted_parts = submitted;
        Ok(Box::new(StagedPart {
            writer: Arc::clone(&self.writer),
            offset,
            bytes,
        }))
    }

    fn complete(mut self: Box<Self>) -> Result<Metadata, CoreError> {
        self.install().map_err(|primary| match self.cancel() {
            Ok(()) => primary,
            Err(cleanup) => append_failure(primary, "semantic staging multipart cleanup", &cleanup),
        })
    }

    fn abort(mut self: Box<Self>) -> Result<(), CoreError> {
        self.cancel()
    }
}

impl Drop for StagedMultipart {
    fn drop(&mut self) {
        // The original mutation retains uncertain physical custody on error.
        let _cleanup = self.cancel();
    }
}

struct StagedPart {
    writer: Arc<Mutex<UploadWriter>>,
    offset: u64,
    bytes: u64,
}

impl SemanticStagingPart for StagedPart {
    fn write(self: Box<Self>, chunks: &[&[u8]]) -> Result<(), CoreError> {
        if staged_file::payload_width(chunks)? != self.bytes {
            return Err(upload_error("part payload differs from its reserved width"));
        }
        let mut state = self
            .writer
            .lock()
            .map_err(|_| upload_error("writer lock poisoned"))?;
        let writer = state
            .file
            .as_mut()
            .ok_or_else(|| upload_error("upload is closed"))?;
        let _position = writer
            .seek(SeekFrom::Start(self.offset))
            .map_err(|error| storage_io("seek part", &error))?;
        for chunk in chunks {
            writer
                .write_all(chunk)
                .map_err(|error| storage_io("write part", &error))?;
        }
        state.completed_parts = state
            .completed_parts
            .checked_add(1)
            .ok_or_else(|| upload_error("completed part count overflow"))?;
        Ok(())
    }
}
