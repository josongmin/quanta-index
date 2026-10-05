//! Bounded source-publication transport. Upload bytes live on disk; the
//! existing sealed batch and its source/journal identity remain unchanged.

use std::fs::{self, File, OpenOptions};
use std::io::{self, BufReader, Read, Seek, SeekFrom, Write};
use std::os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt};
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

struct BodyHashWriter {
    digest: Sha256,
    bytes: u64,
}

impl Write for BodyHashWriter {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.bytes = self.bytes.checked_add(bytes.len() as u64)
            .ok_or_else(|| io::Error::other("source publication size overflow"))?;
        self.digest.update(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> io::Result<()> { Ok(()) }
}

pub fn source_publication_upload_identity(
    batch: &SearchCorpusIngestBatch,
) -> Result<SourcePublicationUploadIdentity, IpcError> {
    let mut writer = BodyHashWriter { digest: Sha256::new(), bytes: 0 };
    ciborium::into_writer(batch, &mut writer)
        .map_err(|error| IpcError::Encode(error.to_string()))?;
    if writer.bytes == 0 || writer.bytes > SOURCE_PUBLICATION_UPLOAD_MAX_BYTES {
        return Err(IpcError::Encode("source publication exceeds staged-body cap".into()));
    }
    Ok(SourcePublicationUploadIdentity {
        body_sha256: writer.digest.finalize().into(), body_bytes: writer.bytes,
    })
}

#[derive(Debug)]
pub enum SourcePublicationUploadError<E> {
    Transport(E),
    Encoding(IpcError),
}

struct PartWriter<'a, F, E> {
    identity: SourcePublicationUploadIdentity,
    offset: u64,
    pending: Vec<u8>,
    emit: &'a mut F,
    failure: Option<E>,
}

impl<F, E> PartWriter<'_, F, E>
where F: FnMut(SourcePublicationUploadPart) -> Result<(), E> {
    fn emit_pending(&mut self) -> io::Result<()> {
        if self.pending.is_empty() { return Ok(()); }
        let bytes = std::mem::take(&mut self.pending);
        let next = self.offset.checked_add(bytes.len() as u64)
            .ok_or_else(|| io::Error::other("source publication offset overflow"))?;
        if let Err(error) = (self.emit)(SourcePublicationUploadPart {
            identity: self.identity, offset: self.offset, bytes,
        }) {
            self.failure = Some(error);
            return Err(io::Error::other("source publication upload refused"));
        }
        self.offset = next;
        Ok(())
    }
}

impl<F, E> Write for PartWriter<'_, F, E>
where F: FnMut(SourcePublicationUploadPart) -> Result<(), E> {
    fn write(&mut self, mut bytes: &[u8]) -> io::Result<usize> {
        let length = bytes.len();
        while !bytes.is_empty() {
            let take = bytes.len().min(SOURCE_PUBLICATION_UPLOAD_PART_BYTES - self.pending.len());
            self.pending.extend_from_slice(&bytes[..take]);
            bytes = &bytes[take..];
            if self.pending.len() == SOURCE_PUBLICATION_UPLOAD_PART_BYTES {
                self.emit_pending()?;
            }
        }
        Ok(length)
    }
    fn flush(&mut self) -> io::Result<()> { self.emit_pending() }
}

/// Serialize directly into bounded parts, without a complete CBOR Vec.
pub fn for_each_source_publication_upload_part<F, E>(
    batch: &SearchCorpusIngestBatch,
    identity: SourcePublicationUploadIdentity,
    mut emit: F,
) -> Result<(), SourcePublicationUploadError<E>>
where F: FnMut(SourcePublicationUploadPart) -> Result<(), E> {
    let mut writer = PartWriter {
        identity, offset: 0, pending: Vec::new(), emit: &mut emit, failure: None,
    };
    let encoded = ciborium::into_writer(batch, &mut writer)
        .map_err(|error| IpcError::Encode(error.to_string()));
    let flushed = encoded.and_then(|()| writer.flush().map_err(|error| IpcError::Encode(error.to_string())));
    if let Some(error) = writer.failure { return Err(SourcePublicationUploadError::Transport(error)); }
    flushed.map_err(SourcePublicationUploadError::Encoding)?;
    if writer.offset != identity.body_bytes {
        return Err(SourcePublicationUploadError::Encoding(IpcError::Encode(
            "source publication changed during upload".into(),
        )));
    }
    Ok(())
}

/// One private staging directory with a disk byte quota and a slot quota.
/// Completed publications remove their upload. Abandoned uploads expire
/// independently of source-event authority; a producer can re-upload them.
pub struct SourcePublicationUploadStore {
    root: PathBuf,
    operations: Mutex<()>,
}

fn invalid(message: impl Into<String>) -> CoreError { CoreError::InvalidContract(message.into()) }
fn storage(error: impl std::fmt::Display) -> CoreError { CoreError::Storage(format!("source upload: {error}")) }
fn busy(message: &str) -> CoreError {
    CoreError::Typed { code: SearchPlaneErrorCodeV2::CatalogBusy, message: message.into() }
}

impl SourcePublicationUploadStore {
    pub fn open(root: impl Into<PathBuf>) -> Result<Self, CoreError> {
        let root = root.into();
        match fs::create_dir(&root) {
            Ok(()) => fs::set_permissions(&root, fs::Permissions::from_mode(0o700)).map_err(storage)?,
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {},
            Err(error) => return Err(storage(error)),
        }
        let metadata = fs::symlink_metadata(&root).map_err(storage)?;
        if !metadata.is_dir() || metadata.file_type().is_symlink()
            || metadata.uid() != rustix::process::geteuid().as_raw()
            || metadata.mode() & 0o077 != 0 {
            return Err(invalid("source upload root must be an owner-private directory"));
        }
        Ok(Self { root, operations: Mutex::new(()) })
    }

    fn path(&self, identity: SourcePublicationUploadIdentity) -> Result<PathBuf, CoreError> {
        if identity.body_bytes == 0 || identity.body_bytes > SOURCE_PUBLICATION_UPLOAD_MAX_BYTES {
            return Err(invalid("source upload body length exceeds its admission bound"));
        }
        let digest = identity.body_sha256.iter().map(|byte| format!("{byte:02x}")).collect::<String>();
        Ok(self.root.join(format!("{digest}-{:016x}.cbor", identity.body_bytes)))
    }

    fn open_body(path: &Path, create: bool) -> Result<File, CoreError> {
        let nofollow = i32::try_from(rustix::fs::OFlags::NOFOLLOW.bits()).map_err(storage)?;
        let file = OpenOptions::new().read(true).write(true).create(create)
            .mode(0o600).custom_flags(nofollow).open(path).map_err(storage)?;
        let meta = file.metadata().map_err(storage)?;
        if !meta.is_file() || meta.nlink() != 1 || meta.uid() != rustix::process::geteuid().as_raw()
            || meta.mode() & 0o077 != 0 {
            return Err(invalid("source upload body must be an owner-private regular file"));
        }
        Ok(file)
    }

    fn inventory(&self) -> Result<(usize, u64), CoreError> {
        let mut count = 0_usize;
        let mut bytes = 0_u64;
        for entry in fs::read_dir(&self.root).map_err(storage)? {
            let entry = entry.map_err(storage)?;
            let meta = fs::symlink_metadata(entry.path()).map_err(storage)?;
            if !meta.is_file() || meta.file_type().is_symlink() || meta.nlink() != 1 {
                return Err(invalid("source upload directory contains a non-regular body"));
            }
            if SystemTime::now().duration_since(meta.modified().map_err(storage)?)
                .is_ok_and(|age| age > STAGING_IDLE_TTL) {
                fs::remove_file(entry.path()).map_err(storage)?;
                continue;
            }
            count = count.checked_add(1).ok_or_else(|| invalid("upload slot overflow"))?;
            bytes = bytes.checked_add(meta.len()).ok_or_else(|| invalid("upload quota overflow"))?;
        }
        Ok((count, bytes))
    }
}

impl SourcePublicationUploadPort for SourcePublicationUploadStore {
    fn stage(&self, part: &SourcePublicationUploadPart, budget: &RequestBudgetV1)
        -> Result<SourcePublicationUploadAck, CoreError> {
        budget.checkpoint("source_upload.stage")?;
        let _operation = self.operations.try_lock().map_err(|_| busy("source upload store busy"))?;
        let path = self.path(part.identity)?;
        if part.bytes.is_empty() || part.bytes.len() > SOURCE_PUBLICATION_UPLOAD_PART_BYTES {
            return Err(invalid("source upload part is empty or exceeds its byte bound"));
        }
        let end = part.offset.checked_add(part.bytes.len() as u64)
            .ok_or_else(|| invalid("source upload offset overflow"))?;
        if end > part.identity.body_bytes { return Err(invalid("source upload part exceeds declared body")); }
        let (slots, total) = self.inventory()?;
        let exists = path.try_exists().map_err(storage)?;
        if !exists && (part.offset != 0 || slots >= STAGING_SLOTS) {
            return Err(invalid("source upload needs its first part or available staging slot"));
        }
        let mut file = Self::open_body(&path, !exists)?;
        let length = file.metadata().map_err(storage)?.len();
        if length > part.identity.body_bytes || part.offset > length {
            return Err(invalid("source upload part is out of order"));
        }
        let overlap = length.saturating_sub(part.offset).min(part.bytes.len() as u64) as usize;
        file.seek(SeekFrom::Start(part.offset)).map_err(storage)?;
        let mut previous = vec![0; overlap];
        file.read_exact(&mut previous).map_err(storage)?;
        if previous != part.bytes[..overlap] { return Err(invalid("source upload retry changes existing bytes")); }
        let growth = end.saturating_sub(length);
        if total.checked_add(growth).is_none_or(|value| value > STAGING_TOTAL_BYTES) {
            return Err(busy("source upload staging disk quota exceeded"));
        }
        file.seek(SeekFrom::Start(part.offset + overlap as u64)).map_err(storage)?;
        file.write_all(&part.bytes[overlap..]).map_err(storage)?;
        file.sync_data().map_err(storage)?;
        File::open(&self.root).and_then(|directory| directory.sync_all()).map_err(storage)?;
        Ok(SourcePublicationUploadAck { identity: part.identity, next_offset: end })
    }

    fn load(&self, identity: SourcePublicationUploadIdentity, budget: &RequestBudgetV1)
        -> Result<SearchCorpusIngestBatch, CoreError> {
        let _operation = self.operations.try_lock().map_err(|_| busy("source upload store busy"))?;
        let mut file = Self::open_body(&self.path(identity)?, false)?;
        if file.metadata().map_err(storage)?.len() != identity.body_bytes {
            return Err(invalid("source upload body is incomplete"));
        }
        let mut digest = Sha256::new();
        let mut scratch = [0_u8; 64 * 1024];
        loop {
            budget.checkpoint("source_upload.verify")?;
            let count = file.read(&mut scratch).map_err(storage)?;
            if count == 0 { break; }
            digest.update(&scratch[..count]);
        }
        let actual: [u8; 32] = digest.finalize().into();
        if actual != identity.body_sha256 { return Err(invalid("source upload body digest mismatch")); }
        file.rewind().map_err(storage)?;
        // Walk before materialization: the same nesting/collection checks as
        // socket decode apply to untrusted staged bytes, without a body Vec.
        let input_len = usize::try_from(identity.body_bytes).map_err(storage)?;
        let text_storage = crate::cbor_preflight::retained_text_budget_reader(BufReader::new(&file), input_len)
            .map_err(|error| invalid(error.to_string()))?;
        if text_storage > SOURCE_PUBLICATION_UPLOAD_MAX_BYTES as usize {
            return Err(invalid("source upload decoded collection storage exceeds its bound"));
        }
        budget.checkpoint("source_upload.decode")?;
        file.rewind().map_err(storage)?;
        let mut reader = BufReader::new(file);
        let batch = ciborium::from_reader(&mut reader).map_err(|error| invalid(error.to_string()))?;
        let mut trailing = [0_u8; 1];
        if reader.read(&mut trailing).map_err(storage)? != 0 {
            return Err(invalid("source upload contains trailing bytes"));
        }
        Ok(batch)
    }

    fn discard(&self, identity: SourcePublicationUploadIdentity) -> Result<(), CoreError> {
        let _operation = self.operations.try_lock().map_err(|_| busy("source upload store busy"))?;
        match fs::remove_file(self.path(identity)?) {
            Ok(()) => File::open(&self.root).and_then(|directory| directory.sync_all()).map_err(storage),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(storage(error)),
        }
    }
}
