//! Bounded source-publication transport. Upload bytes live on disk; the
//! existing sealed batch and its source/journal identity remain unchanged.

use std::fmt::Write as _;
use std::fs::{self, File, OpenOptions};
use std::io::{self, BufReader, Read, Seek, SeekFrom, Write};
use std::os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, SystemTime};

use quanta_index_contract::{
    SOURCE_PUBLICATION_UPLOAD_MAX_BYTES, SOURCE_PUBLICATION_UPLOAD_PART_BYTES,
    SearchCorpusIngestBatch, SearchPlaneErrorCodeV2, SourcePublicationUploadAck,
    SourcePublicationUploadIdentity, SourcePublicationUploadPart,
};
use quanta_index_core::{CoreError, RequestBudgetV1, SourcePublicationUploadPort};
use sha2::{Digest, Sha256};

use crate::IpcError;

/// Keep the ordinary aggregate decoded-request cap unchanged. Large source
/// publications use bounded upload requests and one small commit request.
pub const SOURCE_PUBLICATION_INLINE_BYTES: u64 = 64 * 1024 * 1024;
const STAGING_TOTAL_BYTES: u64 = 1024 * 1024 * 1024;
const STAGING_SLOTS: usize = 8;
const STAGING_IDLE_TTL: Duration = Duration::from_secs(24 * 60 * 60);
const DECODE_READ_BYTES: usize = 8192;

struct BodyHashWriter {
    digest: Sha256,
    bytes: u64,
}

impl Write for BodyHashWriter {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.bytes = self
            .bytes
            .checked_add(u64::try_from(bytes.len()).map_err(io::Error::other)?)
            .ok_or_else(|| io::Error::other("source publication size overflow"))?;
        self.digest.update(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

pub fn source_publication_upload_identity(
    batch: &SearchCorpusIngestBatch,
) -> Result<SourcePublicationUploadIdentity, IpcError> {
    let mut writer = BodyHashWriter {
        digest: Sha256::new(),
        bytes: 0,
    };
    ciborium::into_writer(batch, &mut writer)
        .map_err(|error| IpcError::Encode(error.to_string()))?;
    if writer.bytes == 0 || writer.bytes > SOURCE_PUBLICATION_UPLOAD_MAX_BYTES {
        return Err(IpcError::Encode(
            "source publication exceeds staged-body cap".into(),
        ));
    }
    Ok(SourcePublicationUploadIdentity {
        body_sha256: writer.digest.finalize().into(),
        body_bytes: writer.bytes,
    })
}

#[derive(Debug)]
pub enum SourcePublicationUploadError<E> {
    Transport(E),
    Encoding(IpcError),
}

impl<E: std::fmt::Display> std::fmt::Display for SourcePublicationUploadError<E> {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Transport(error) => write!(formatter, "source upload transport: {error}"),
            Self::Encoding(error) => write!(formatter, "source upload encoding: {error}"),
        }
    }
}

impl<E: std::error::Error + 'static> std::error::Error for SourcePublicationUploadError<E> {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Transport(error) => Some(error),
            Self::Encoding(error) => Some(error),
        }
    }
}

struct PartWriter<'a, F, E> {
    identity: SourcePublicationUploadIdentity,
    offset: u64,
    pending: Vec<u8>,
    emit: &'a mut F,
    failure: Option<E>,
}

impl<F, E> PartWriter<'_, F, E>
where
    F: FnMut(SourcePublicationUploadPart) -> Result<(), E>,
{
    fn emit_pending(&mut self) -> io::Result<()> {
        if self.pending.is_empty() {
            return Ok(());
        }
        let bytes = std::mem::take(&mut self.pending);
        let next = self
            .offset
            .checked_add(u64::try_from(bytes.len()).map_err(io::Error::other)?)
            .ok_or_else(|| io::Error::other("source publication offset overflow"))?;
        if let Err(error) = (self.emit)(SourcePublicationUploadPart {
            identity: self.identity,
            offset: self.offset,
            bytes,
        }) {
            self.failure = Some(error);
            return Err(io::Error::other("source publication upload refused"));
        }
        self.offset = next;
        Ok(())
    }
}

impl<F, E> Write for PartWriter<'_, F, E>
where
    F: FnMut(SourcePublicationUploadPart) -> Result<(), E>,
{
    fn write(&mut self, mut bytes: &[u8]) -> io::Result<usize> {
        let length = bytes.len();
        while !bytes.is_empty() {
            let available = SOURCE_PUBLICATION_UPLOAD_PART_BYTES
                .checked_sub(self.pending.len())
                .ok_or_else(|| io::Error::other("source publication part buffer exceeds bound"))?;
            let take = bytes.len().min(available);
            let (prefix, remaining) = bytes
                .split_at_checked(take)
                .ok_or_else(|| io::Error::other("source publication part split exceeds input"))?;
            self.pending.extend_from_slice(prefix);
            bytes = remaining;
            if self.pending.len() == SOURCE_PUBLICATION_UPLOAD_PART_BYTES {
                self.emit_pending()?;
            }
        }
        Ok(length)
    }
    fn flush(&mut self) -> io::Result<()> {
        self.emit_pending()
    }
}

/// Serialize directly into bounded parts, without a complete CBOR Vec.
pub fn for_each_source_publication_upload_part<F, E>(
    batch: &SearchCorpusIngestBatch,
    identity: SourcePublicationUploadIdentity,
    mut emit: F,
) -> Result<(), SourcePublicationUploadError<E>>
where
    F: FnMut(SourcePublicationUploadPart) -> Result<(), E>,
{
    let mut writer = PartWriter {
        identity,
        offset: 0,
        pending: Vec::new(),
        emit: &mut emit,
        failure: None,
    };
    let encoded = ciborium::into_writer(batch, &mut writer)
        .map_err(|error| IpcError::Encode(error.to_string()));
    let flushed = encoded.and_then(|()| {
        writer
            .flush()
            .map_err(|error| IpcError::Encode(error.to_string()))
    });
    if let Some(error) = writer.failure {
        return Err(SourcePublicationUploadError::Transport(error));
    }
    flushed.map_err(SourcePublicationUploadError::Encoding)?;
    if writer.offset != identity.body_bytes {
        return Err(SourcePublicationUploadError::Encoding(IpcError::Encode(
            "source publication changed during upload".into(),
        )));
    }
    Ok(())
}

/// One private staging directory with a disk byte quota and a slot quota.
///
/// Completed publications remove their upload. Abandoned uploads expire
/// independently of source-event authority; a producer can re-upload them.
pub struct SourcePublicationUploadStore {
    root: PathBuf,
    max_body_bytes: u64,
    operations: Mutex<()>,
}

fn invalid(message: impl Into<String>) -> CoreError {
    CoreError::InvalidContract(message.into())
}
fn storage(error: impl std::fmt::Display) -> CoreError {
    CoreError::Storage(format!("source upload: {error}"))
}
fn busy(message: &str) -> CoreError {
    CoreError::Typed {
        code: SearchPlaneErrorCodeV2::CatalogBusy,
        message: message.into(),
    }
}

/// Check cancellation before each bounded buffer refill.
///
/// Keep the typed interruption across CBOR's
/// I/O error wrapper instead of reporting a cancelled request as invalid input.
struct BudgetReader<'a, R> {
    inner: R,
    budget: &'a RequestBudgetV1,
    checkpoint: &'static str,
    interruption: Option<CoreError>,
}

impl<'a, R> BudgetReader<'a, R> {
    fn new(inner: R, budget: &'a RequestBudgetV1, checkpoint: &'static str) -> Self {
        Self {
            inner,
            budget,
            checkpoint,
            interruption: None,
        }
    }

    fn preserve_interruption(&mut self, otherwise: CoreError) -> CoreError {
        self.interruption
            .take()
            .map_or(otherwise, std::convert::identity)
    }
}

impl<R: Read> Read for BudgetReader<'_, R> {
    fn read(&mut self, bytes: &mut [u8]) -> io::Result<usize> {
        if let Err(interruption) = self.budget.checkpoint(self.checkpoint) {
            self.interruption = Some(interruption);
            return Err(io::Error::other("source upload interrupted"));
        }
        // BufReader may bypass its buffer for large read requests.
        let take = bytes.len().min(DECODE_READ_BYTES);
        self.inner.read(
            bytes
                .get_mut(..take)
                .ok_or_else(|| io::Error::other("source upload read exceeds input"))?,
        )
    }
}

impl SourcePublicationUploadStore {
    pub fn open(root: impl Into<PathBuf>, max_body_bytes: u64) -> Result<Self, CoreError> {
        if max_body_bytes == 0 || max_body_bytes > SOURCE_PUBLICATION_UPLOAD_MAX_BYTES {
            return Err(invalid(
                "source upload configured limit must be in 1..=512 MiB",
            ));
        }
        let root = root.into();
        match fs::DirBuilder::new().mode(0o700).create(&root) {
            Ok(()) => {}
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
            Err(error) => return Err(storage(error)),
        }
        let metadata = fs::symlink_metadata(&root).map_err(storage)?;
        if !metadata.is_dir()
            || metadata.file_type().is_symlink()
            || metadata.uid() != rustix::process::geteuid().as_raw()
            || metadata.mode() & 0o077 != 0
        {
            return Err(invalid(
                "source upload root must be an owner-private directory",
            ));
        }
        Ok(Self {
            root,
            max_body_bytes,
            operations: Mutex::new(()),
        })
    }

    fn path(&self, identity: SourcePublicationUploadIdentity) -> Result<PathBuf, CoreError> {
        if identity.body_bytes == 0 || identity.body_bytes > self.max_body_bytes {
            return Err(invalid(
                "source upload body length exceeds its admission bound",
            ));
        }
        let mut digest = String::new();
        for byte in identity.body_sha256 {
            write!(digest, "{byte:02x}").map_err(storage)?;
        }
        Ok(self
            .root
            .join(format!("{digest}-{:016x}.cbor", identity.body_bytes)))
    }

    fn open_body(path: &Path, create: bool) -> Result<File, CoreError> {
        let nofollow = i32::try_from(rustix::fs::OFlags::NOFOLLOW.bits()).map_err(storage)?;
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(create)
            .mode(0o600)
            .custom_flags(nofollow)
            .open(path)
            .map_err(storage)?;
        let meta = file.metadata().map_err(storage)?;
        if !meta.is_file()
            || meta.nlink() != 1
            || meta.uid() != rustix::process::geteuid().as_raw()
            || meta.mode() & 0o077 != 0
        {
            return Err(invalid(
                "source upload body must be an owner-private regular file",
            ));
        }
        Ok(file)
    }

    fn inventory(&self) -> Result<(usize, u64), CoreError> {
        let mut count = 0_usize;
        let mut bytes = 0_u64;
        for entry in fs::read_dir(&self.root).map_err(storage)? {
            let entry = entry.map_err(storage)?;
            let meta = fs::symlink_metadata(entry.path()).map_err(storage)?;
            if !meta.is_file()
                || meta.file_type().is_symlink()
                || meta.nlink() != 1
                || meta.uid() != rustix::process::geteuid().as_raw()
                || meta.mode() & 0o077 != 0
            {
                return Err(invalid(
                    "source upload directory contains a non-private regular body",
                ));
            }
            if SystemTime::now()
                .duration_since(meta.modified().map_err(storage)?)
                .is_ok_and(|age| age > STAGING_IDLE_TTL)
            {
                fs::remove_file(entry.path()).map_err(storage)?;
                continue;
            }
            count = count
                .checked_add(1)
                .ok_or_else(|| invalid("upload slot overflow"))?;
            bytes = bytes
                .checked_add(meta.len())
                .ok_or_else(|| invalid("upload quota overflow"))?;
        }
        Ok((count, bytes))
    }

    fn decode_body<R: Read + Seek>(
        &self,
        file: &mut R,
        input_len: usize,
        budget: &RequestBudgetV1,
    ) -> Result<SearchCorpusIngestBatch, CoreError> {
        // The guard sits below buffering: work can consume at most one
        // bounded refill before it observes cancellation again.
        let text_storage = {
            let mut reader = BufReader::with_capacity(
                DECODE_READ_BYTES,
                BudgetReader::new(&mut *file, budget, "source_upload.preflight"),
            );
            crate::cbor_preflight::retained_text_budget_reader(&mut reader, input_len).map_err(
                |error| {
                    reader
                        .get_mut()
                        .preserve_interruption(invalid(error.to_string()))
                },
            )?
        };
        budget.checkpoint("source_upload.preflight")?;
        if text_storage > usize::try_from(self.max_body_bytes).map_err(storage)? {
            return Err(invalid(
                "source upload decoded collection storage exceeds its bound",
            ));
        }
        budget.checkpoint("source_upload.decode")?;
        file.rewind().map_err(storage)?;
        let mut reader = BufReader::with_capacity(
            DECODE_READ_BYTES,
            BudgetReader::new(file, budget, "source_upload.decode"),
        );
        let batch = ciborium::from_reader(&mut reader).map_err(|error| {
            reader
                .get_mut()
                .preserve_interruption(invalid(error.to_string()))
        })?;
        budget.checkpoint("source_upload.decode")?;
        let mut trailing = [0_u8; 1];
        if reader
            .read(&mut trailing)
            .map_err(|error| reader.get_mut().preserve_interruption(storage(error)))?
            != 0
        {
            return Err(invalid("source upload contains trailing bytes"));
        }
        budget.checkpoint("source_upload.decode")?;
        Ok(batch)
    }
}

impl SourcePublicationUploadPort for SourcePublicationUploadStore {
    fn stage(
        &self,
        part: &SourcePublicationUploadPart,
        budget: &RequestBudgetV1,
    ) -> Result<SourcePublicationUploadAck, CoreError> {
        budget.checkpoint("source_upload.stage")?;
        let _operation = self
            .operations
            .try_lock()
            .map_err(|_error| busy("source upload store busy"))?;
        let path = self.path(part.identity)?;
        if part.bytes.is_empty() || part.bytes.len() > SOURCE_PUBLICATION_UPLOAD_PART_BYTES {
            return Err(invalid(
                "source upload part is empty or exceeds its byte bound",
            ));
        }
        let part_length = u64::try_from(part.bytes.len()).map_err(storage)?;
        let end = part
            .offset
            .checked_add(part_length)
            .ok_or_else(|| invalid("source upload offset overflow"))?;
        if end > part.identity.body_bytes {
            return Err(invalid("source upload part exceeds declared body"));
        }
        let (slots, total) = self.inventory()?;
        let exists = path.try_exists().map_err(storage)?;
        if !exists && (part.offset != 0 || slots >= STAGING_SLOTS) {
            return Err(invalid(
                "source upload needs its first part or available staging slot",
            ));
        }
        // Admit growth before creating a body: refused requests must not
        // consume slots with empty files. The store mutex fences inventory
        // and append together, including retry-overlap admission.
        let mut existing = exists.then(|| Self::open_body(&path, false)).transpose()?;
        let length = existing
            .as_ref()
            .map(|file| file.metadata().map(|meta| meta.len()))
            .transpose()
            .map_err(storage)?
            .unwrap_or(0);
        if length > part.identity.body_bytes || part.offset > length {
            return Err(invalid("source upload part is out of order"));
        }
        let overlap_bytes = length.saturating_sub(part.offset).min(part_length);
        let overlap = usize::try_from(overlap_bytes).map_err(storage)?;
        if let Some(file) = &mut existing {
            let _offset = file.seek(SeekFrom::Start(part.offset)).map_err(storage)?;
            let mut previous = vec![0; overlap];
            file.read_exact(&mut previous).map_err(storage)?;
            if Some(previous.as_slice()) != part.bytes.get(..overlap) {
                return Err(invalid("source upload retry changes existing bytes"));
            }
        }
        let growth = end.saturating_sub(length);
        if total
            .checked_add(growth)
            .is_none_or(|value| value > STAGING_TOTAL_BYTES)
        {
            return Err(busy("source upload staging disk quota exceeded"));
        }
        let mut file = match existing {
            Some(file) => file,
            None => Self::open_body(&path, true)?,
        };
        let write_offset = part
            .offset
            .checked_add(overlap_bytes)
            .ok_or_else(|| invalid("source upload overlap offset overflow"))?;
        let _offset = file.seek(SeekFrom::Start(write_offset)).map_err(storage)?;
        let remaining = part
            .bytes
            .get(overlap..)
            .ok_or_else(|| invalid("source upload overlap exceeds part"))?;
        file.write_all(remaining).map_err(storage)?;
        file.sync_data().map_err(storage)?;
        File::open(&self.root)
            .and_then(|directory| directory.sync_all())
            .map_err(storage)?;
        Ok(SourcePublicationUploadAck {
            identity: part.identity,
            next_offset: end,
        })
    }

    fn load(
        &self,
        identity: SourcePublicationUploadIdentity,
        budget: &RequestBudgetV1,
    ) -> Result<SearchCorpusIngestBatch, CoreError> {
        budget.checkpoint("source_upload.load")?;
        let _operation = self
            .operations
            .try_lock()
            .map_err(|_error| busy("source upload store busy"))?;
        let mut file = Self::open_body(&self.path(identity)?, false)?;
        if file.metadata().map_err(storage)?.len() != identity.body_bytes {
            return Err(invalid("source upload body is incomplete"));
        }
        let mut digest = Sha256::new();
        let mut scratch = vec![0_u8; 64 * 1024];
        loop {
            budget.checkpoint("source_upload.verify")?;
            let count = file.read(&mut scratch).map_err(storage)?;
            if count == 0 {
                break;
            }
            digest.update(
                scratch
                    .get(..count)
                    .ok_or_else(|| invalid("source upload read exceeds scratch"))?,
            );
        }
        let actual: [u8; 32] = digest.finalize().into();
        if actual != identity.body_sha256 {
            return Err(invalid("source upload body digest mismatch"));
        }
        file.rewind().map_err(storage)?;
        // Walk before materialization: the same nesting/collection checks as
        // socket decode apply to untrusted staged bytes, without a body Vec.
        let input_len = usize::try_from(identity.body_bytes).map_err(storage)?;
        self.decode_body(&mut file, input_len, budget)
    }

    fn discard(&self, identity: SourcePublicationUploadIdentity) -> Result<(), CoreError> {
        let _operation = self
            .operations
            .try_lock()
            .map_err(|_error| busy("source upload store busy"))?;
        match fs::remove_file(self.path(identity)?) {
            Ok(()) => File::open(&self.root)
                .and_then(|directory| directory.sync_all())
                .map_err(storage),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(storage(error)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use quanta_index_contract::{
        BatchIngestMode, ManifestGeneration, RepoId, RevisionId, SourcePublicationEvent,
    };
    use std::time::Instant;

    fn budget() -> RequestBudgetV1 {
        RequestBudgetV1::until(
            Instant::now()
                .checked_add(Duration::from_secs(60))
                .expect("fixture deadline must be representable"),
        )
    }

    fn batch() -> SearchCorpusIngestBatch {
        SearchCorpusIngestBatch {
            source_event: SourcePublicationEvent {
                stream_id: "upload-test".into(),
                event_id: "event-1".into(),
                expected_base_event_id: None,
                payload_sha256: [0; 32],
            },
            repo_id: RepoId::new("repo").expect("fixture"),
            revision_id: RevisionId::new("revision").expect("fixture"),
            generation: ManifestGeneration::new(1),
            base_generation: None,
            manifest_digest: "manifest".into(),
            batch_digest: String::new(),
            mode: BatchIngestMode::ReplaceGeneration,
            bundle_payload: Some(vec![7; SOURCE_PUBLICATION_UPLOAD_PART_BYTES]),
            clear_surfaces: Vec::new(),
            replace_scopes: Vec::new(),
            tombstone_scopes: Vec::new(),
            semantic_replace_scopes: Vec::new(),
            semantic_tombstone_scopes: Vec::new(),
            seal: true,
        }
    }

    struct CancelOnRead {
        bytes: io::Cursor<Vec<u8>>,
        cancellation: quanta_index_core::CancelHandleV1,
        cancel_during_decode: bool,
        decoding: bool,
        selected_reads: usize,
        selected_bytes: usize,
    }

    impl Read for CancelOnRead {
        fn read(&mut self, bytes: &mut [u8]) -> io::Result<usize> {
            let count = self.bytes.read(bytes)?;
            if self.decoding == self.cancel_during_decode {
                self.selected_reads = self
                    .selected_reads
                    .checked_add(1)
                    .ok_or_else(|| io::Error::other("fixture read count overflow"))?;
                self.selected_bytes = self
                    .selected_bytes
                    .checked_add(count)
                    .ok_or_else(|| io::Error::other("fixture byte count overflow"))?;
                self.cancellation.cancel();
            }
            Ok(count)
        }
    }

    impl Seek for CancelOnRead {
        fn seek(&mut self, position: SeekFrom) -> io::Result<u64> {
            if position == SeekFrom::Start(0) {
                self.decoding = true;
            }
            self.bytes.seek(position)
        }
    }

    struct CancelOnEof {
        bytes: io::Cursor<Vec<u8>>,
        cancellation: quanta_index_core::CancelHandleV1,
        eof_reads: usize,
    }

    impl Read for CancelOnEof {
        fn read(&mut self, bytes: &mut [u8]) -> io::Result<usize> {
            let count = self.bytes.read(bytes)?;
            if count == 0 {
                self.eof_reads = self
                    .eof_reads
                    .checked_add(1)
                    .expect("fixture EOF read count fits usize");
                self.cancellation.cancel();
            }
            Ok(count)
        }
    }

    impl Seek for CancelOnEof {
        fn seek(&mut self, position: SeekFrom) -> io::Result<u64> {
            self.bytes.seek(position)
        }
    }

    #[test]
    fn staged_publication_decode_refuses_cancellation_during_final_eof_read() {
        let root = tempfile::tempdir().expect("tempdir");
        let store = SourcePublicationUploadStore::open(
            root.path().join("uploads"),
            SOURCE_PUBLICATION_UPLOAD_MAX_BYTES,
        )
        .expect("open");
        let bytes = crate::encode_cbor_payload(&batch()).expect("valid body");
        let budget = RequestBudgetV1::unbounded();
        let mut source = CancelOnEof {
            bytes: io::Cursor::new(bytes.clone()),
            cancellation: budget.cancel_handle(),
            eof_reads: 0,
        };
        let error = store
            .decode_body(&mut source, bytes.len(), &budget)
            .expect_err("cancellation at final EOF must not return a batch");
        let (code, message) = error.into_search_plane_wire();
        assert_eq!(code, SearchPlaneErrorCodeV2::RequestCancelled);
        assert!(message.contains("source_upload.decode"));
        assert_eq!(source.eof_reads, 1);
    }

    #[test]
    fn staged_publication_budget_reader_bounds_reads_that_bypass_buffering() {
        let budget = RequestBudgetV1::unbounded();
        let source = CancelOnRead {
            bytes: io::Cursor::new(vec![1; 64 * 1024]),
            cancellation: budget.cancel_handle(),
            cancel_during_decode: false,
            decoding: false,
            selected_reads: 0,
            selected_bytes: 0,
        };
        let mut reader = BufReader::with_capacity(
            8192,
            BudgetReader::new(source, &budget, "source_upload.decode"),
        );
        let mut output = vec![0; 64 * 1024];
        assert_eq!(reader.read(&mut output).expect("first bounded read"), 8192);
        assert!(reader.read(&mut output).is_err());
        let guarded = reader.get_mut();
        let (code, message) = guarded
            .preserve_interruption(invalid("expected cancellation"))
            .into_search_plane_wire();
        assert_eq!(code, SearchPlaneErrorCodeV2::RequestCancelled);
        assert!(message.contains("source_upload.decode"));
        assert_eq!(guarded.inner.selected_reads, 1);
        assert_eq!(guarded.inner.selected_bytes, 8192);
    }

    #[test]
    fn staged_publication_decode_observes_cancellation_between_reads_and_before_return() {
        let root = tempfile::tempdir().expect("tempdir");
        let store = SourcePublicationUploadStore::open(
            root.path().join("uploads"),
            SOURCE_PUBLICATION_UPLOAD_MAX_BYTES,
        )
        .expect("open");
        for large in [false, true] {
            let mut batch = batch();
            if !large {
                batch.bundle_payload = None;
            }
            let bytes = crate::encode_cbor_payload(&batch).expect("valid body");
            let input_len = bytes.len();
            assert_eq!(
                store
                    .decode_body(
                        &mut io::Cursor::new(&bytes),
                        input_len,
                        &RequestBudgetV1::unbounded(),
                    )
                    .expect("uncancelled body remains valid"),
                batch,
            );
            for cancel_during_decode in [false, true] {
                let budget = RequestBudgetV1::unbounded();
                let mut reader = CancelOnRead {
                    bytes: io::Cursor::new(bytes.clone()),
                    cancellation: budget.cancel_handle(),
                    cancel_during_decode,
                    decoding: false,
                    selected_reads: 0,
                    selected_bytes: 0,
                };
                let error = store
                    .decode_body(&mut reader, input_len, &budget)
                    .expect_err("cancelled decode must not return a batch");
                let (code, message) = error.into_search_plane_wire();
                assert_eq!(code, SearchPlaneErrorCodeV2::RequestCancelled);
                let checkpoint = if cancel_during_decode {
                    "source_upload.decode"
                } else {
                    "source_upload.preflight"
                };
                assert!(message.contains(checkpoint));
                assert_eq!(reader.selected_reads, 1, "no read after cancellation");
                assert!(reader.selected_bytes <= 8192, "bounded read quantum");
            }
        }
    }

    #[test]
    fn staged_publication_load_refuses_expired_budget_before_opening_a_body() {
        let root = tempfile::tempdir().expect("tempdir");
        let store = SourcePublicationUploadStore::open(
            root.path().join("uploads"),
            SOURCE_PUBLICATION_UPLOAD_MAX_BYTES,
        )
        .expect("open");
        let identity = SourcePublicationUploadIdentity {
            body_sha256: [1; 32],
            body_bytes: 1,
        };
        let expired = RequestBudgetV1::until(
            Instant::now()
                .checked_sub(Duration::from_secs(1))
                .expect("fixture expired deadline must be representable"),
        );
        let error = store.load(identity, &expired).expect_err("expired request");
        assert!(matches!(
            error,
            CoreError::Typed {
                code: SearchPlaneErrorCodeV2::RequestDeadlineExceeded,
                ..
            }
        ));
        assert_eq!(store.inventory().expect("inventory"), (0, 0));
    }

    #[test]
    fn configured_body_limit_refuses_before_creating_files() {
        let root = tempfile::tempdir().expect("root");
        let path = root.path().join("uploads");
        assert!(SourcePublicationUploadStore::open(&path, 0).is_err());
        assert!(!path.exists());
        let store = SourcePublicationUploadStore::open(&path, 16).expect("open");
        let part = SourcePublicationUploadPart {
            identity: SourcePublicationUploadIdentity {
                body_sha256: [1; 32],
                body_bytes: 17,
            },
            offset: 0,
            bytes: vec![1],
        };
        assert!(store.stage(&part, &budget()).is_err());
        assert_eq!(std::fs::read_dir(&path).expect("dir").count(), 0);
    }

    #[test]
    fn staged_publication_round_trips_after_store_restart_without_a_body_buffer() {
        let root = tempfile::tempdir().expect("tempdir");
        let path = root.path().join("uploads");
        let store = SourcePublicationUploadStore::open(&path, SOURCE_PUBLICATION_UPLOAD_MAX_BYTES)
            .expect("open");
        let batch = batch();
        let identity = source_publication_upload_identity(&batch).expect("identity");
        let mut parts = 0_usize;
        for_each_source_publication_upload_part(
            &batch,
            identity,
            |part| -> Result<(), CoreError> {
                assert!(part.bytes.len() <= SOURCE_PUBLICATION_UPLOAD_PART_BYTES);
                assert!(
                    crate::encode_request(&part).expect("bounded request").len()
                        < 2 * SOURCE_PUBLICATION_UPLOAD_PART_BYTES
                );
                let ack = store.stage(&part, &budget())?;
                assert_eq!(
                    ack.next_offset,
                    part.offset
                        .checked_add(u64::try_from(part.bytes.len()).expect("part length"))
                        .expect("part end")
                );
                parts = parts.checked_add(1).expect("part count");
                Ok(())
            },
        )
        .expect("upload");
        assert!(parts > 1);
        drop(store);
        let restarted =
            SourcePublicationUploadStore::open(&path, SOURCE_PUBLICATION_UPLOAD_MAX_BYTES)
                .expect("reopen");
        assert_eq!(
            restarted.load(identity, &budget()).expect("complete body"),
            batch
        );
        restarted.discard(identity).expect("discard");
        assert!(restarted.load(identity, &budget()).is_err());
    }

    #[test]
    fn staged_publication_rejects_gaps_conflicts_incomplete_bodies_and_bad_digests() {
        let root = tempfile::tempdir().expect("tempdir");
        let store = SourcePublicationUploadStore::open(
            root.path().join("uploads"),
            SOURCE_PUBLICATION_UPLOAD_MAX_BYTES,
        )
        .expect("open");
        let identity = SourcePublicationUploadIdentity {
            body_sha256: Sha256::digest(b"abcdef").into(),
            body_bytes: 6,
        };
        let first = SourcePublicationUploadPart {
            identity,
            offset: 0,
            bytes: b"abc".to_vec(),
        };
        assert!(
            store
                .stage(
                    &SourcePublicationUploadPart {
                        offset: 3,
                        bytes: b"def".to_vec(),
                        ..first
                    },
                    &budget()
                )
                .is_err()
        );
        let _ack = store.stage(&first, &budget()).expect("first");
        assert_eq!(
            store
                .stage(&first, &budget())
                .expect("lost ACK retry")
                .next_offset,
            3
        );
        assert!(store.load(identity, &budget()).is_err());
        assert!(
            store
                .stage(
                    &SourcePublicationUploadPart {
                        bytes: b"xyz".to_vec(),
                        ..first
                    },
                    &budget()
                )
                .is_err()
        );
        assert!(
            store
                .stage(
                    &SourcePublicationUploadPart {
                        offset: 4,
                        bytes: b"ef".to_vec(),
                        ..first
                    },
                    &budget()
                )
                .is_err()
        );
        let _ack = store
            .stage(
                &SourcePublicationUploadPart {
                    offset: 3,
                    bytes: b"deg".to_vec(),
                    ..first
                },
                &budget(),
            )
            .expect("complete wrong body");
        let error = store
            .load(identity, &budget())
            .expect_err("digest mismatch");
        assert!(error.to_string().contains("digest mismatch"));
    }

    #[test]
    fn staged_publication_resumes_a_part_interrupted_before_ack_and_bounds_slots() {
        let root = tempfile::tempdir().expect("tempdir");
        let store = SourcePublicationUploadStore::open(
            root.path().join("uploads"),
            SOURCE_PUBLICATION_UPLOAD_MAX_BYTES,
        )
        .expect("open");
        let identity = SourcePublicationUploadIdentity {
            body_sha256: [1; 32],
            body_bytes: 8,
        };
        let first = SourcePublicationUploadPart {
            identity,
            offset: 0,
            bytes: b"ab".to_vec(),
        };
        let _ack = store
            .stage(&first, &budget())
            .expect("partial durable write");
        let ack = store
            .stage(
                &SourcePublicationUploadPart {
                    bytes: b"abcd".to_vec(),
                    ..first
                },
                &budget(),
            )
            .expect("resume matching prefix");
        assert_eq!(ack.next_offset, 4);
        for index in 2..=STAGING_SLOTS {
            let _ack = store
                .stage(
                    &SourcePublicationUploadPart {
                        identity: SourcePublicationUploadIdentity {
                            body_sha256: [u8::try_from(index).expect("slot index"); 32],
                            body_bytes: 8,
                        },
                        offset: 0,
                        bytes: vec![1],
                    },
                    &budget(),
                )
                .expect("available slot");
        }
        assert!(
            store
                .stage(
                    &SourcePublicationUploadPart {
                        identity: SourcePublicationUploadIdentity {
                            body_sha256: [99; 32],
                            body_bytes: 8
                        },
                        offset: 0,
                        bytes: vec![1],
                    },
                    &budget()
                )
                .is_err()
        );
        assert!(
            store
                .stage(
                    &SourcePublicationUploadPart {
                        identity: SourcePublicationUploadIdentity {
                            body_sha256: [99; 32],
                            body_bytes: SOURCE_PUBLICATION_UPLOAD_MAX_BYTES + 1
                        },
                        offset: 0,
                        bytes: vec![1],
                    },
                    &budget()
                )
                .is_err()
        );
    }

    #[test]
    fn staged_publication_rejects_malformed_cbor_before_materialization() {
        let root = tempfile::tempdir().expect("tempdir");
        let store = SourcePublicationUploadStore::open(
            root.path().join("uploads"),
            SOURCE_PUBLICATION_UPLOAD_MAX_BYTES,
        )
        .expect("open");
        // Fixed hostile encodings: truncated array, two top-level values,
        // impossible collection length, and depth beyond the decoder limit.
        let mut nested = vec![0x81; 257];
        nested.push(0);
        for bytes in [
            vec![0x82, 0],
            vec![0, 0],
            vec![0x9a, 0xff, 0xff, 0xff, 0xff],
            nested,
        ] {
            let identity = SourcePublicationUploadIdentity {
                body_sha256: Sha256::digest(&bytes).into(),
                body_bytes: u64::try_from(bytes.len()).expect("body length"),
            };
            let _ack = store
                .stage(
                    &SourcePublicationUploadPart {
                        identity,
                        offset: 0,
                        bytes,
                    },
                    &budget(),
                )
                .expect("stage opaque bytes");
            assert!(store.load(identity, &budget()).is_err());
            store.discard(identity).expect("discard rejected body");
        }
    }

    #[test]
    fn staged_publication_disk_admission_does_not_leave_an_empty_slot() {
        let root = tempfile::tempdir().expect("tempdir");
        let store = SourcePublicationUploadStore::open(
            root.path().join("uploads"),
            SOURCE_PUBLICATION_UPLOAD_MAX_BYTES,
        )
        .expect("open");
        // Sparse files exercise the logical byte quota without writing 1 GiB.
        for index in 1..=2 {
            let identity = SourcePublicationUploadIdentity {
                body_sha256: [index; 32],
                body_bytes: SOURCE_PUBLICATION_UPLOAD_MAX_BYTES,
            };
            SourcePublicationUploadStore::open_body(&store.path(identity).expect("path"), true)
                .expect("fixture")
                .set_len(SOURCE_PUBLICATION_UPLOAD_MAX_BYTES)
                .expect("sparse quota fixture");
        }
        let refused = SourcePublicationUploadIdentity {
            body_sha256: [3; 32],
            body_bytes: 1,
        };
        let error = store
            .stage(
                &SourcePublicationUploadPart {
                    identity: refused,
                    offset: 0,
                    bytes: vec![0],
                },
                &budget(),
            )
            .expect_err("disk quota");
        assert!(error.to_string().contains("disk quota"));
        assert!(!store.path(refused).expect("path").exists());
        assert_eq!(
            store.inventory().expect("inventory"),
            (2, STAGING_TOTAL_BYTES)
        );
    }

    #[test]
    fn staged_publication_refuses_links_and_cancelled_requests() {
        let root = tempfile::tempdir().expect("tempdir");
        let store = SourcePublicationUploadStore::open(
            root.path().join("uploads"),
            SOURCE_PUBLICATION_UPLOAD_MAX_BYTES,
        )
        .expect("open");
        let identity = SourcePublicationUploadIdentity {
            body_sha256: [1; 32],
            body_bytes: 1,
        };
        let part = SourcePublicationUploadPart {
            identity,
            offset: 0,
            bytes: vec![0],
        };
        let expired = RequestBudgetV1::until(
            Instant::now()
                .checked_sub(Duration::from_secs(1))
                .expect("fixture expired deadline must be representable"),
        );
        assert!(store.stage(&part, &expired).is_err());
        assert!(store.inventory().expect("inventory").0 == 0);
        let outside = root.path().join("outside");
        fs::write(&outside, b"original").expect("outside fixture");
        std::os::unix::fs::symlink(&outside, store.path(identity).expect("path")).expect("symlink");
        assert!(store.stage(&part, &budget()).is_err());
        assert!(store.load(identity, &budget()).is_err());
        assert_eq!(
            fs::read(&outside).expect("unchanged external file"),
            b"original"
        );
    }
}
