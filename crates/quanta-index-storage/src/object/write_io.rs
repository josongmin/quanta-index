//! Actual writer ownership covers cleanup, original close and shared settlement.

use std::{fs::File, path::Path};

use quanta_index_core::index::{IndexError, ObjectWriteTicket};

use super::{TempFile, publish_temp_no_replace};

pub(super) struct ObjectWriter {
    file: Option<File>,
    temporary: TempFile,
    ticket: Option<Box<dyn ObjectWriteTicket>>,
}

impl ObjectWriter {
    pub(super) fn new(
        file: File,
        temporary: std::path::PathBuf,
        ticket: Option<Box<dyn ObjectWriteTicket>>,
    ) -> Self {
        Self {
            file: Some(file),
            temporary: TempFile(temporary),
            ticket,
        }
    }

    pub(super) fn file(&self) -> Result<&File, IndexError> {
        self.file
            .as_ref()
            .ok_or_else(|| IndexError::BackendViolation("writer FD retired".into()))
    }

    pub(super) fn file_mut(&mut self) -> Result<&mut File, IndexError> {
        self.file
            .as_mut()
            .ok_or_else(|| IndexError::BackendViolation("writer FD retired".into()))
    }

    pub(super) fn created(&mut self) -> Result<(), IndexError> {
        if let Some(ticket) = self.ticket.as_mut() {
            ticket.created(
                self.file
                    .as_ref()
                    .ok_or_else(|| IndexError::BackendViolation("writer FD absent".into()))?,
            )?;
        }
        Ok(())
    }

    pub(super) fn publish(
        &mut self,
        final_path: &Path,
        written: &std::fs::Metadata,
    ) -> Result<bool, IndexError> {
        publish_temp_no_replace(&mut self.temporary, final_path, written)
    }

    /// Confirm actual original close. Published outputs retain the independent
    /// pin until the caller has completed the shared sync protocol.
    pub(super) fn retire(mut self) -> Result<Option<Box<dyn ObjectWriteTicket>>, IndexError> {
        let file = self
            .file
            .take()
            .ok_or_else(|| IndexError::BackendViolation("writer retired twice".into()))?;
        if let Some(mut ticket) = self.ticket.take() {
            if self.temporary.0.as_os_str().is_empty() {
                ticket.retire_writer(file)?;
                Ok(Some(ticket))
            } else {
                // Disarm raw TempFile cleanup even if qualified cleanup fails.
                self.temporary.0.clear();
                ticket.cancel(file)?;
                Ok(None)
            }
        } else {
            drop(file);
            Ok(None)
        }
    }
}

impl Drop for ObjectWriter {
    fn drop(&mut self) {
        if let Some(file) = self.file.take() {
            if let Some(mut ticket) = self.ticket.take() {
                let published = self.temporary.0.as_os_str().is_empty();
                // The physical owner alone performs qualified cleanup.
                self.temporary.0.clear();
                let _closed = if published {
                    ticket.retire_writer(file)
                } else {
                    ticket.cancel(file)
                };
            } else {
                drop(file);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        fs,
        io::Write as _,
        sync::{
            Arc,
            atomic::{AtomicUsize, Ordering},
        },
    };

    struct Refusal(Arc<AtomicUsize>);
    impl ObjectWriteTicket for Refusal {
        fn creation_started(&mut self) -> Result<(), IndexError> {
            Ok(())
        }
        fn created(&mut self, _: &File) -> Result<(), IndexError> {
            Ok(())
        }
        fn uncreated(&mut self) -> Result<(), IndexError> {
            Err(IndexError::ResourceLimited)
        }
        fn retire_writer(&mut self, writer: File) -> Result<(), IndexError> {
            drop(writer);
            Ok(())
        }
        fn cancel(&mut self, writer: File) -> Result<(), IndexError> {
            drop(writer);
            let _previous = self.0.fetch_add(1, Ordering::SeqCst);
            Err(IndexError::ResourceLimited)
        }
        fn synced(self: Box<Self>) -> Result<(), IndexError> {
            Err(IndexError::ResourceLimited)
        }
    }
    #[test]
    fn qualified_cleanup_refusal_never_falls_back_to_tempfile_delete()
    -> Result<(), Box<dyn std::error::Error>> {
        let directory = tempfile::tempdir()?;
        let path = directory.path().join(".tmp-owned");
        let mut file = File::create(&path)?;
        file.write_all(b"retained-temp")?;
        let calls = Arc::new(AtomicUsize::new(0));
        let owner = ObjectWriter::new(
            file,
            path.clone(),
            Some(Box::new(Refusal(Arc::clone(&calls)))),
        );
        drop(owner);
        if fs::read(path)? != b"retained-temp" || calls.load(Ordering::SeqCst) != 1 {
            return Err(std::io::Error::other(
                "refused physical cleanup deleted bytes or ran twice",
            )
            .into());
        }
        Ok(())
    }
}
