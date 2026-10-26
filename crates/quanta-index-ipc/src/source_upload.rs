//! Bounded source-publication transport. Upload bytes live on disk; the
//! existing sealed batch and its source/journal identity remain unchanged.

use std::ffi::{OsStr, OsString};
use std::fmt::Write as _;
use std::fs::{self, File};
use std::io::{self, BufReader, Read, Seek, SeekFrom, Write};
use std::os::fd::AsFd;
use std::os::unix::ffi::OsStringExt as _;
use std::os::unix::fs::MetadataExt;
use std::path::{Component, Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime};

use quanta_index_contract::{
    SOURCE_PUBLICATION_UPLOAD_MAX_BYTES, SOURCE_PUBLICATION_UPLOAD_PART_BYTES,
    SearchCorpusIngestBatch, SearchPlaneErrorCodeV2, SourcePublicationUploadAck,
    SourcePublicationUploadIdentity, SourcePublicationUploadPart,
};
use quanta_index_core::publication_admission::PublicationControlReader;
use quanta_index_core::publication_capacity::InodeKey;
use quanta_index_core::{
    CoreError, RequestBudgetV1, SourcePublicationUploadFileAdmissionPort,
    SourcePublicationUploadPort,
};
use rustix::fs::{AtFlags, Dir, FileType, Mode, OFlags, mkdirat, open, openat, statat, unlinkat};
use rustix::io::Errno;
use sha2::{Digest, Sha256};

use crate::IpcError;

/// Keep the ordinary aggregate decoded-request cap unchanged. Large source
/// publications use bounded upload requests and one small commit request.
pub const SOURCE_PUBLICATION_INLINE_BYTES: u64 = 64 * 1024 * 1024;
const STAGING_TOTAL_BYTES: u64 = 1024 * 1024 * 1024;
/// Transport bodies held by the one upload store under a leased root.
pub const SOURCE_PUBLICATION_UPLOAD_STAGING_SLOTS: usize = 8;
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
    directory: File,
    parent: Box<dyn SourcePublicationUploadParent>,
    component: OsString,
    max_body_bytes: u64,
    file_admission: Option<Arc<dyn SourcePublicationUploadFileAdmissionPort>>,
    operations: Mutex<()>,
}

/// Filesystem custody retained and revalidated at each upload transition.
/// A leased parent must also validate its held ownership authority.
pub trait SourcePublicationUploadParent: AsFd + Send + Sync {
    fn require_current_v1(&self) -> Result<(), CoreError>;
}

impl SourcePublicationUploadParent for File {
    fn require_current_v1(&self) -> Result<(), CoreError> {
        if !self.metadata().map_err(storage)?.is_dir() {
            return Err(invalid("source upload parent is not a directory"));
        }
        Ok(())
    }
}

fn invalid(message: impl Into<String>) -> CoreError {
    CoreError::InvalidContract(message.into())
}
fn storage(error: impl std::fmt::Display) -> CoreError {
    CoreError::Storage(format!("source upload: {error}"))
}
fn admitted_io_error(context: &str, error: &io::Error) -> CoreError {
    let mut cause: &(dyn std::error::Error + 'static) = error;
    loop {
        if let Some(core) = cause.downcast_ref::<CoreError>() {
            return core.clone();
        }
        let next = cause
            .downcast_ref::<io::Error>()
            .and_then(io::Error::get_ref)
            .map(|inner| -> &(dyn std::error::Error + 'static) { inner })
            .or_else(|| cause.source());
        match next {
            Some(inner) => cause = inner,
            None => return CoreError::storage_io(context, error),
        }
    }
}
fn attach_retirement(primary: CoreError, retirement: &CoreError) -> CoreError {
    let suffix = format!("; reader/writer retirement also failed: {retirement}");
    match primary {
        CoreError::InvalidContract(message) => {
            CoreError::InvalidContract(format!("{message}{suffix}"))
        }
        CoreError::Typed { code, message } => CoreError::Typed {
            code,
            message: format!("{message}{suffix}"),
        },
        CoreError::NotReady(message) => CoreError::NotReady(format!("{message}{suffix}")),
        CoreError::NotImplemented(message) => {
            CoreError::NotImplemented(format!("{message}{suffix}"))
        }
        CoreError::NotFound(message) => CoreError::NotFound(format!("{message}{suffix}")),
        CoreError::Storage(message) => CoreError::Storage(format!("{message}{suffix}")),
        CoreError::StorageIo {
            kind,
            raw_os_error,
            message,
        } => CoreError::StorageIo {
            kind,
            raw_os_error,
            message: format!("{message}{suffix}"),
        },
    }
}
fn finish_with_retirement<T>(
    result: Result<T, CoreError>,
    retired: Result<(), CoreError>,
) -> Result<T, CoreError> {
    match (result, retired) {
        (Ok(value), Ok(())) => Ok(value),
        (Err(primary), Ok(())) => Err(primary),
        (Ok(_), Err(retirement)) => Err(retirement),
        (Err(primary), Err(retirement)) => Err(attach_retirement(primary, &retirement)),
    }
}

enum BodyReader {
    Raw(File),
    Admitted(Box<dyn PublicationControlReader>),
}

impl BodyReader {
    fn metadata(&self) -> Result<fs::Metadata, CoreError> {
        match self {
            Self::Raw(file) => file.metadata().map_err(storage),
            Self::Admitted(reader) => reader.metadata(),
        }
    }
    fn sync_posix(&self) -> Result<(), CoreError> {
        match self {
            Self::Raw(file) => file.sync_data().map_err(storage),
            Self::Admitted(reader) => reader.sync_posix(),
        }
    }
    fn retire(self) -> Result<(), CoreError> {
        match self {
            Self::Raw(file) => {
                drop(file);
                Ok(())
            }
            Self::Admitted(reader) => reader.retire(),
        }
    }
}

impl Read for BodyReader {
    fn read(&mut self, bytes: &mut [u8]) -> io::Result<usize> {
        match self {
            Self::Raw(file) => file.read(bytes),
            Self::Admitted(reader) => reader.read(bytes),
        }
    }
}
impl Seek for BodyReader {
    fn seek(&mut self, position: SeekFrom) -> io::Result<u64> {
        match self {
            Self::Raw(file) => file.seek(position),
            Self::Admitted(reader) => reader.seek(position),
        }
    }
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
    io_failure: Option<CoreError>,
}

impl<'a, R> BudgetReader<'a, R> {
    fn new(inner: R, budget: &'a RequestBudgetV1, checkpoint: &'static str) -> Self {
        Self {
            inner,
            budget,
            checkpoint,
            interruption: None,
            io_failure: None,
        }
    }

    fn preserve_interruption(&mut self, otherwise: CoreError) -> CoreError {
        self.interruption
            .take()
            .or_else(|| self.io_failure.take())
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
        let result = self.inner.read(
            bytes
                .get_mut(..take)
                .ok_or_else(|| io::Error::other("source upload read exceeds input"))?,
        );
        if let Err(error) = &result {
            self.io_failure = Some(admitted_io_error("source upload body read", error));
        }
        result
    }
}

impl SourcePublicationUploadStore {
    pub fn open(root: impl Into<PathBuf>, max_body_bytes: u64) -> Result<Self, CoreError> {
        let root = root.into();
        let parent_path = root
            .parent()
            .filter(|path| !path.as_os_str().is_empty())
            .unwrap_or_else(|| Path::new("."));
        let component = root
            .file_name()
            .ok_or_else(|| invalid("source upload root needs a final component"))?;
        let parent = open(
            parent_path,
            OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
            Mode::empty(),
        )
        .map(File::from)
        .map_err(storage)?;
        Self::open_at(parent, parent_path, component, max_body_bytes)
    }

    /// Open one upload directory below an owned parent directory.
    ///
    /// The parent custody stays inside this store until its directory FD is
    /// closed. The local staging quota assumes one store per upload root;
    /// production assembly holds the state-root lease and constructs only one.
    pub fn open_at<P: SourcePublicationUploadParent + 'static>(
        parent: P,
        parent_path: &Path,
        component: &OsStr,
        max_body_bytes: u64,
    ) -> Result<Self, CoreError> {
        Self::open_at_inner(parent, parent_path, component, max_body_bytes, None)
    }

    /// Install physical admission before the upload directory is created or
    /// its existing bodies are inventoried.
    pub fn open_at_with_file_admission<P: SourcePublicationUploadParent + 'static>(
        parent: P,
        parent_path: &Path,
        component: &OsStr,
        max_body_bytes: u64,
        file_admission: Arc<dyn SourcePublicationUploadFileAdmissionPort>,
    ) -> Result<Self, CoreError> {
        Self::open_at_inner(
            parent,
            parent_path,
            component,
            max_body_bytes,
            Some(file_admission),
        )
    }

    fn open_at_inner<P: SourcePublicationUploadParent + 'static>(
        parent: P,
        parent_path: &Path,
        component: &OsStr,
        max_body_bytes: u64,
        file_admission: Option<Arc<dyn SourcePublicationUploadFileAdmissionPort>>,
    ) -> Result<Self, CoreError> {
        if max_body_bytes == 0 || max_body_bytes > SOURCE_PUBLICATION_UPLOAD_MAX_BYTES {
            return Err(invalid(
                "source upload configured limit must be in 1..=512 MiB",
            ));
        }
        let mut components = Path::new(component).components();
        if !matches!(components.next(), Some(Component::Normal(name)) if name == component)
            || components.next().is_some()
        {
            return Err(invalid("source upload root needs one normal component"));
        }
        parent.require_current_v1()?;
        let opened_parent = parent
            .as_fd()
            .try_clone_to_owned()
            .map(File::from)
            .map_err(storage)?;
        let opened_parent = opened_parent.metadata().map_err(storage)?;
        let named_parent = fs::symlink_metadata(parent_path).map_err(storage)?;
        if !opened_parent.is_dir()
            || !named_parent.is_dir()
            || named_parent.file_type().is_symlink()
            || (opened_parent.dev(), opened_parent.ino())
                != (named_parent.dev(), named_parent.ino())
        {
            return Err(invalid("source upload parent identity changed"));
        }
        let root = parent_path.join(component);
        if let Some(admission) = &file_admission {
            admission.validate_root_and_reconcile(&root)?;
        }
        match mkdirat(&parent, Path::new(component), Mode::from_raw_mode(0o700)) {
            Ok(()) | Err(Errno::EXIST) => {}
            Err(error) => return Err(storage(error)),
        }
        let directory = openat(
            &parent,
            Path::new(component),
            OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
            Mode::empty(),
        )
        .map(File::from)
        .map_err(storage)?;
        let metadata = directory.metadata().map_err(storage)?;
        if !metadata.is_dir()
            || metadata.uid() != rustix::process::geteuid().as_raw()
            || metadata.mode() & 0o077 != 0
        {
            return Err(invalid(
                "source upload root must be an owner-private directory",
            ));
        }
        let store = Self {
            root,
            directory,
            parent: Box::new(parent),
            component: component.to_os_string(),
            max_body_bytes,
            file_admission,
            operations: Mutex::new(()),
        };
        store.require_current_root()?;
        // An ACK may be issued after the first body arrives. Persist the
        // upload directory's own entry before any such request can run.
        store.directory.sync_all().map_err(storage)?;
        let parent_file = store
            .parent
            .as_fd()
            .try_clone_to_owned()
            .map(File::from)
            .map_err(storage)?;
        parent_file.sync_all().map_err(storage)?;
        store.require_current_root()?;
        Ok(store)
    }

    fn require_current_root(&self) -> Result<(), CoreError> {
        self.parent.require_current_v1()?;
        let opened = self.directory.metadata().map_err(storage)?;
        let parent_entry = statat(
            &self.parent,
            Path::new(&self.component),
            AtFlags::SYMLINK_NOFOLLOW,
        )
        .map_err(storage)?;
        let path_entry = fs::symlink_metadata(&self.root).map_err(storage)?;
        let expected = (opened.dev(), opened.ino());
        let parent_device = u64::try_from(parent_entry.st_dev).map_err(storage)?;
        if !opened.is_dir()
            || FileType::from_raw_mode(parent_entry.st_mode) != FileType::Directory
            || (parent_device, parent_entry.st_ino) != expected
            || !path_entry.is_dir()
            || path_entry.file_type().is_symlink()
            || (path_entry.dev(), path_entry.ino()) != expected
            || opened.uid() != rustix::process::geteuid().as_raw()
            || opened.mode() & 0o077 != 0
        {
            return Err(invalid("source upload root identity or privacy changed"));
        }
        self.parent.require_current_v1()?;
        Ok(())
    }

    fn reconcile_admission(&self) -> Result<(), CoreError> {
        if let Some(admission) = &self.file_admission {
            admission.validate_root_and_reconcile(&self.root)?;
        }
        Ok(())
    }

    fn body_name(&self, identity: SourcePublicationUploadIdentity) -> Result<OsString, CoreError> {
        if identity.body_bytes == 0 || identity.body_bytes > self.max_body_bytes {
            return Err(invalid(
                "source upload body length exceeds its admission bound",
            ));
        }
        let mut digest = String::new();
        for byte in identity.body_sha256 {
            write!(digest, "{byte:02x}").map_err(storage)?;
        }
        Ok(OsString::from(format!(
            "{digest}-{:016x}.cbor",
            identity.body_bytes
        )))
    }

    #[cfg(test)]
    fn path(&self, identity: SourcePublicationUploadIdentity) -> Result<PathBuf, CoreError> {
        Ok(self.root.join(self.body_name(identity)?))
    }

    fn body_exists(&self, name: &OsStr) -> Result<bool, CoreError> {
        match statat(&self.directory, Path::new(name), AtFlags::SYMLINK_NOFOLLOW) {
            Ok(entry) => {
                if FileType::from_raw_mode(entry.st_mode) != FileType::RegularFile {
                    return Err(invalid("source upload body must be a regular file"));
                }
                Ok(true)
            }
            Err(Errno::NOENT) => Ok(false),
            Err(error) => Err(storage(error)),
        }
    }

    fn open_body(&self, name: &OsStr, create: bool) -> Result<File, CoreError> {
        if !create && !self.body_exists(name)? {
            return Err(storage("source upload body is absent"));
        }
        let mut flags = OFlags::RDWR | OFlags::CLOEXEC | OFlags::NOFOLLOW | OFlags::NONBLOCK;
        if create {
            flags |= OFlags::CREATE | OFlags::EXCL;
        }
        let file = openat(
            &self.directory,
            Path::new(name),
            flags,
            Mode::from_raw_mode(0o600),
        )
        .map(File::from)
        .map_err(storage)?;
        self.require_body_entry(name, &file)?;
        Ok(file)
    }

    fn open_body_reader(&self, name: &OsStr) -> Result<BodyReader, CoreError> {
        let Some(admission) = &self.file_admission else {
            return self.open_body(name, false).map(BodyReader::Raw);
        };
        let path = self.root.join(name);
        let ticket = admission.begin_reader_open(&path)?;
        let file = openat(
            &self.directory,
            Path::new(name),
            OFlags::RDONLY | OFlags::CLOEXEC | OFlags::NOFOLLOW | OFlags::NONBLOCK,
            Mode::empty(),
        )
        .map(File::from)
        .map_err(storage)?;
        let reader = ticket.opened_owned(file)?;
        if let Err(primary) = reader
            .metadata()
            .and_then(|meta| self.require_body_metadata(name, &meta))
        {
            return finish_with_retirement(Err(primary), reader.retire());
        }
        // A successful authentication keeps the exact admitted reader open.
        Ok(BodyReader::Admitted(reader))
    }

    fn require_body_entry(&self, name: &OsStr, file: &File) -> Result<(), CoreError> {
        let meta = file.metadata().map_err(storage)?;
        self.require_body_metadata(name, &meta)
    }

    fn require_body_metadata(&self, name: &OsStr, meta: &fs::Metadata) -> Result<(), CoreError> {
        let entry =
            statat(&self.directory, Path::new(name), AtFlags::SYMLINK_NOFOLLOW).map_err(storage)?;
        let device = u64::try_from(entry.st_dev).map_err(storage)?;
        Self::require_private_body_metadata(meta)?;
        if FileType::from_raw_mode(entry.st_mode) != FileType::RegularFile
            || (device, entry.st_ino) != (meta.dev(), meta.ino())
            || u64::try_from(entry.st_size).map_err(storage)? != meta.len()
            || u64::from(entry.st_nlink) != meta.nlink()
        {
            return Err(invalid(
                "source upload body identity or privacy changed during operation",
            ));
        }
        Ok(())
    }

    fn require_private_body_metadata(meta: &fs::Metadata) -> Result<(), CoreError> {
        if !meta.is_file()
            || meta.nlink() != 1
            || meta.uid() != rustix::process::geteuid().as_raw()
            || meta.mode() & 0o077 != 0
        {
            return Err(invalid(
                "source upload body is not an owner-private regular file",
            ));
        }
        Ok(())
    }

    fn delete_admitted_body(&self, name: &OsStr, meta: &fs::Metadata) -> Result<(), CoreError> {
        let admission = self
            .file_admission
            .as_ref()
            .ok_or_else(|| invalid("missing upload file admission"))?;
        let inode = InodeKey {
            device: meta.dev(),
            inode: meta.ino(),
        };
        admission.delete_file_and_sync(&self.root.join(name), inode, meta.len())
    }

    fn inventory(&self, budget: &RequestBudgetV1) -> Result<(usize, u64), CoreError> {
        let mut count = 0_usize;
        let mut bytes = 0_u64;
        let mut seen = 0_usize;
        let mut entries = Dir::read_from(&self.directory).map_err(storage)?;
        while let Some(entry) = entries.read() {
            budget.checkpoint("source_upload.inventory")?;
            let entry = entry.map_err(storage)?;
            let raw = entry.file_name().to_bytes();
            if raw == b"." || raw == b".." {
                continue;
            }
            seen = seen
                .checked_add(1)
                .ok_or_else(|| busy("source upload inventory entry overflow"))?;
            if seen > SOURCE_PUBLICATION_UPLOAD_STAGING_SLOTS {
                return Err(busy(
                    "source upload inventory exceeds staging slot frontier",
                ));
            }
            let name = OsString::from_vec(raw.to_vec());
            let file = self.open_body_reader(&name)?;
            let outcome = (|| {
                let meta = file.metadata()?;
                if !meta.is_file()
                    || meta.nlink() != 1
                    || meta.uid() != rustix::process::geteuid().as_raw()
                    || meta.mode() & 0o077 != 0
                {
                    return Err(invalid(
                        "source upload directory contains a non-private regular body",
                    ));
                }
                let expired = SystemTime::now()
                    .duration_since(meta.modified().map_err(storage)?)
                    .is_ok_and(|age| age > STAGING_IDLE_TTL);
                if expired && self.file_admission.is_some() {
                    self.require_current_root()?;
                    self.delete_admitted_body(&name, &meta)?;
                }
                Ok((meta.len(), expired))
            })();
            let (length, expired) = finish_with_retirement(outcome, file.retire())?;
            if expired {
                if self.file_admission.is_none() {
                    self.require_current_root()?;
                    unlinkat(&self.directory, Path::new(&name), AtFlags::empty())
                        .map_err(storage)?;
                    self.directory.sync_all().map_err(storage)?;
                }
                continue;
            }
            count = count
                .checked_add(1)
                .ok_or_else(|| invalid("upload slot overflow"))?;
            bytes = bytes
                .checked_add(length)
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
        file.rewind()
            .map_err(|error| admitted_io_error("source upload decode rewind", &error))?;
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

    fn stage_admitted(
        &self,
        part: &SourcePublicationUploadPart,
        name: &OsStr,
        exists: bool,
        total: u64,
        end: u64,
        budget: &RequestBudgetV1,
    ) -> Result<SourcePublicationUploadAck, CoreError> {
        let admission = self
            .file_admission
            .as_ref()
            .ok_or_else(|| invalid("missing upload file admission"))?;
        let mut reader = exists.then(|| self.open_body_reader(name)).transpose()?;
        let observed = (|| {
            let original = reader.as_ref().map(BodyReader::metadata).transpose()?;
            let length = original.as_ref().map_or(0, fs::Metadata::len);
            let original_inode = original.as_ref().map(|meta| InodeKey {
                device: meta.dev(),
                inode: meta.ino(),
            });
            if length > part.identity.body_bytes || part.offset > length {
                return Err(invalid("source upload part is out of order"));
            }
            let part_length = u64::try_from(part.bytes.len()).map_err(storage)?;
            let overlap_bytes = length.saturating_sub(part.offset).min(part_length);
            let overlap = usize::try_from(overlap_bytes).map_err(storage)?;
            if let Some(opened) = &mut reader {
                let _offset = opened
                    .seek(SeekFrom::Start(part.offset))
                    .map_err(|error| admitted_io_error("source upload overlap seek", &error))?;
                let mut previous = vec![0; overlap];
                opened
                    .read_exact(&mut previous)
                    .map_err(|error| admitted_io_error("source upload overlap read", &error))?;
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
            Ok((length, overlap, growth, original_inode))
        })();
        let (length, overlap, growth, original_inode) = match observed {
            Ok(value) => value,
            Err(primary) => {
                return finish_with_retirement(
                    Err(primary),
                    reader.map_or(Ok(()), BodyReader::retire),
                );
            }
        };
        if let Err(primary) = budget
            .checkpoint("source_upload.prewrite")
            .and_then(|()| self.require_current_root())
        {
            return finish_with_retirement(Err(primary), reader.map_or(Ok(()), BodyReader::retire));
        }
        if growth == 0 {
            let result = (|| {
                let opened = reader
                    .as_ref()
                    .ok_or_else(|| invalid("zero-growth upload has no body"))?;
                opened.sync_posix()?;
                self.directory.sync_all().map_err(storage)?;
                self.require_body_metadata(name, &opened.metadata()?)?;
                self.require_current_root()
            })();
            finish_with_retirement(result, reader.map_or(Ok(()), BodyReader::retire))?;
        } else {
            if let Some(opened) = reader {
                opened.retire()?;
            }
            let mut writer = admission.begin_write(
                &self.root.join(name),
                part.identity.body_bytes,
                exists.then_some(length),
                end,
            )?;
            let result = (|| {
                let opened = writer.metadata()?;
                if let Some(inode) = original_inode
                    && (opened.dev(), opened.ino(), opened.len())
                        != (inode.device, inode.inode, length)
                {
                    return Err(invalid("source upload writer differs from overlap reader"));
                }
                let write_offset = part
                    .offset
                    .checked_add(u64::try_from(overlap).map_err(storage)?)
                    .ok_or_else(|| invalid("source upload overlap offset overflow"))?;
                let _offset = writer
                    .seek(SeekFrom::Start(write_offset))
                    .map_err(|error| admitted_io_error("source upload write seek", &error))?;
                let remaining = part
                    .bytes
                    .get(overlap..)
                    .ok_or_else(|| invalid("source upload overlap exceeds part"))?;
                writer
                    .write_all(remaining)
                    .map_err(|error| admitted_io_error("source upload write", &error))?;
                let meta = writer.metadata()?;
                if meta.len() != end {
                    return Err(invalid(
                        "source upload writer length differs from target end",
                    ));
                }
                Self::require_private_body_metadata(&meta)?;
                if exists {
                    self.require_body_metadata(name, &meta)?;
                }
                self.require_current_root()?;
                Ok(meta)
            })();
            let meta = match result {
                Ok(meta) => meta,
                Err(primary) => {
                    return finish_with_retirement(
                        Err(primary),
                        writer.retire_preserving_pending(),
                    );
                }
            };
            writer.sync_and_settle()?;
            self.require_body_metadata(name, &meta)?;
            self.require_current_root()?;
        }
        budget.checkpoint("source_upload.ack")?;
        Ok(SourcePublicationUploadAck {
            identity: part.identity,
            next_offset: end,
        })
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
        self.reconcile_admission()?;
        self.require_current_root()?;
        let name = self.body_name(part.identity)?;
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
        let (slots, total) = self.inventory(budget)?;
        let exists = self.body_exists(&name)?;
        if !exists {
            if part.offset != 0 {
                return Err(invalid("source upload needs its first part"));
            }
            if slots >= SOURCE_PUBLICATION_UPLOAD_STAGING_SLOTS {
                return Err(busy("source upload staging slot quota exceeded"));
            }
        }
        if self.file_admission.is_some() {
            return self.stage_admitted(part, &name, exists, total, end, budget);
        }
        // Admit growth before creating a body: refused requests must not
        // consume slots with empty files. The store mutex fences inventory
        // and append together, including retry-overlap admission.
        let mut existing = exists.then(|| self.open_body(&name, false)).transpose()?;
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
        budget.checkpoint("source_upload.prewrite")?;
        self.require_current_root()?;
        let mut file = match existing {
            Some(file) => file,
            None => self.open_body(&name, true)?,
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
        self.directory.sync_all().map_err(storage)?;
        self.require_body_entry(&name, &file)?;
        self.require_current_root()?;
        budget.checkpoint("source_upload.ack")?;
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
        self.reconcile_admission()?;
        self.require_current_root()?;
        let name = self.body_name(identity)?;
        let mut file = self.open_body_reader(&name)?;
        let result = (|| {
            if file.metadata()?.len() != identity.body_bytes {
                return Err(invalid("source upload body is incomplete"));
            }
            let mut digest = Sha256::new();
            let mut scratch = vec![0_u8; 64 * 1024];
            loop {
                budget.checkpoint("source_upload.verify")?;
                let count = file
                    .read(&mut scratch)
                    .map_err(|error| admitted_io_error("source upload verify read", &error))?;
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
            file.rewind()
                .map_err(|error| admitted_io_error("source upload verify rewind", &error))?;
            // Walk before materialization without buffering a complete body.
            let input_len = usize::try_from(identity.body_bytes).map_err(storage)?;
            let batch = self.decode_body(&mut file, input_len, budget)?;
            self.require_body_metadata(&name, &file.metadata()?)?;
            self.require_current_root()?;
            budget.checkpoint("source_upload.load")?;
            Ok(batch)
        })();
        finish_with_retirement(result, file.retire())
    }

    fn discard(&self, identity: SourcePublicationUploadIdentity) -> Result<(), CoreError> {
        let _operation = self
            .operations
            .try_lock()
            .map_err(|_error| busy("source upload store busy"))?;
        self.reconcile_admission()?;
        self.require_current_root()?;
        let name = self.body_name(identity)?;
        if !self.body_exists(&name)? {
            return Ok(());
        }
        let file = self.open_body_reader(&name)?;
        let result = (|| {
            let meta = file.metadata()?;
            self.require_current_root()?;
            if self.file_admission.is_some() {
                self.delete_admitted_body(&name, &meta)?;
            } else {
                unlinkat(&self.directory, Path::new(&name), AtFlags::empty()).map_err(storage)?;
                self.directory.sync_all().map_err(storage)?;
            }
            self.require_current_root()
        })();
        finish_with_retirement(result, file.retire())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use quanta_index_contract::{
        BatchIngestMode, ManifestGeneration, RepoId, RevisionId, SourcePublicationEvent,
    };
    use std::time::Instant;

    type TestResult = Result<(), Box<dyn std::error::Error>>;

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
        assert_eq!(store.inventory(&budget()).expect("inventory"), (0, 0));
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
        for index in 2..=SOURCE_PUBLICATION_UPLOAD_STAGING_SLOTS {
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
        let next = SourcePublicationUploadPart {
            identity: SourcePublicationUploadIdentity {
                body_sha256: [99; 32],
                body_bytes: 8,
            },
            offset: 0,
            bytes: vec![1],
        };
        let refused = store.stage(&next, &budget()).expect_err("slot quota");
        assert_eq!(
            refused.into_search_plane_wire().0,
            SearchPlaneErrorCodeV2::CatalogBusy,
        );
        assert!(!store.path(next.identity).expect("path").exists());
        let gap = SourcePublicationUploadPart {
            offset: 1,
            ..next.clone()
        };
        let invalid = store
            .stage(&gap, &budget())
            .expect_err("missing first part");
        assert_eq!(
            invalid.into_search_plane_wire().0,
            SearchPlaneErrorCodeV2::InvalidRequest
        );
        store.discard(identity).expect("release one slot");
        let _ack = store
            .stage(&next, &budget())
            .expect("retry after slot release");
        assert_eq!(
            store.inventory(&budget()).expect("inventory").0,
            SOURCE_PUBLICATION_UPLOAD_STAGING_SLOTS
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
            store
                .open_body(&store.body_name(identity).expect("name"), true)
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
            store.inventory(&budget()).expect("inventory"),
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
        assert!(store.inventory(&budget()).expect("inventory").0 == 0);
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

    #[test]
    fn upload_constructor_refuses_parent_mismatch_and_nonlocal_components_before_creation()
    -> TestResult {
        let named_parent = tempfile::tempdir()?;
        let opened_parent = tempfile::tempdir()?;
        let result = SourcePublicationUploadStore::open_at(
            File::open(opened_parent.path())?,
            named_parent.path(),
            OsStr::new("uploads"),
            16,
        );
        if result.is_ok()
            || named_parent.path().join("uploads").exists()
            || opened_parent.path().join("uploads").exists()
        {
            return Err("mismatched parent custody created an upload directory".into());
        }
        for component in [".", "..", "nested/uploads", "/uploads"] {
            if SourcePublicationUploadStore::open_at(
                File::open(named_parent.path())?,
                named_parent.path(),
                OsStr::new(component),
                16,
            )
            .is_ok()
            {
                return Err("an upload root escaped its single-component contract".into());
            }
        }
        if fs::read_dir(named_parent.path())?.next().is_some() {
            return Err("invalid upload components changed the parent directory".into());
        }
        Ok(())
    }

    #[test]
    fn replaced_upload_root_refuses_stage_load_and_discard_without_touching_replacement()
    -> TestResult {
        use std::os::unix::fs::PermissionsExt as _;

        let parent = tempfile::tempdir()?;
        let root = parent.path().join("uploads");
        let store = SourcePublicationUploadStore::open(&root, SOURCE_PUBLICATION_UPLOAD_MAX_BYTES)?;
        let mut batch = batch();
        batch.bundle_payload = None;
        let mut encoded = Vec::new();
        ciborium::into_writer(&batch, &mut encoded)?;
        let identity = SourcePublicationUploadIdentity {
            body_sha256: Sha256::digest(&encoded).into(),
            body_bytes: u64::try_from(encoded.len())?,
        };
        let part = SourcePublicationUploadPart {
            identity,
            offset: 0,
            bytes: encoded,
        };
        let _ack = store.stage(&part, &budget())?;
        let original = parent.path().join("original-uploads");
        fs::rename(&root, &original)?;
        fs::create_dir(&root)?;
        fs::set_permissions(&root, fs::Permissions::from_mode(0o700))?;
        fs::write(root.join("untouched"), b"replacement")?;

        if store.stage(&part, &budget()).is_ok()
            || store.load(identity, &budget()).is_ok()
            || store.discard(identity).is_ok()
        {
            return Err("a replaced upload root accepted an operation".into());
        }
        if fs::read(root.join("untouched"))? != b"replacement"
            || fs::read(original.join(store.body_name(identity)?))? != part.bytes
        {
            return Err("root replacement refusal changed a stored body".into());
        }
        Ok(())
    }

    #[test]
    fn inventory_frontier_refuses_before_creating_a_new_upload() -> TestResult {
        let parent = tempfile::tempdir()?;
        let store = SourcePublicationUploadStore::open(
            parent.path().join("uploads"),
            SOURCE_PUBLICATION_UPLOAD_MAX_BYTES,
        )?;
        for index in 0..=SOURCE_PUBLICATION_UPLOAD_STAGING_SLOTS {
            let digest_byte = u8::try_from(index.checked_add(1).ok_or("fixture overflow")?)?;
            let identity = SourcePublicationUploadIdentity {
                body_sha256: [digest_byte; 32],
                body_bytes: 1,
            };
            let _file = store.open_body(&store.body_name(identity)?, true)?;
        }
        let requested = SourcePublicationUploadIdentity {
            body_sha256: [99; 32],
            body_bytes: 1,
        };
        let error = store.stage(
            &SourcePublicationUploadPart {
                identity: requested,
                offset: 0,
                bytes: vec![7],
            },
            &budget(),
        );
        if !matches!(
            error,
            Err(CoreError::Typed {
                code: SearchPlaneErrorCodeV2::CatalogBusy,
                ..
            })
        ) || store.path(requested)?.exists()
        {
            return Err("inventory frontier accepted or created a new upload".into());
        }
        Ok(())
    }

    #[test]
    fn fifo_body_refuses_without_waiting_for_a_peer() -> TestResult {
        let parent = tempfile::tempdir()?;
        let store = SourcePublicationUploadStore::open(
            parent.path().join("uploads"),
            SOURCE_PUBLICATION_UPLOAD_MAX_BYTES,
        )?;
        let identity = SourcePublicationUploadIdentity {
            body_sha256: [1; 32],
            body_bytes: 1,
        };
        // rustix 1.1.4 does not expose mkfifoat on Apple targets.
        let created = std::process::Command::new("mkfifo")
            .args(["-m", "600"])
            .arg(store.path(identity)?)
            .status()?;
        if !created.success() {
            return Err("failed to create the FIFO fixture".into());
        }
        if store
            .stage(
                &SourcePublicationUploadPart {
                    identity,
                    offset: 0,
                    bytes: vec![1],
                },
                &budget(),
            )
            .is_ok()
            || store.load(identity, &budget()).is_ok()
        {
            return Err("a FIFO was accepted as an upload body".into());
        }
        Ok(())
    }

    #[test]
    fn opened_body_revalidation_refuses_privacy_and_link_drift() -> TestResult {
        use std::os::unix::fs::PermissionsExt as _;

        let parent = tempfile::tempdir()?;
        let store = SourcePublicationUploadStore::open(parent.path().join("uploads"), 16)?;
        let identity = SourcePublicationUploadIdentity {
            body_sha256: [3; 32],
            body_bytes: 1,
        };
        let name = store.body_name(identity)?;
        let mut file = store.open_body(&name, true)?;
        file.write_all(&[7])?;
        store.require_body_entry(&name, &file)?;

        // Both path and FD still name the original inode. Privacy must be
        // checked again at the final ACK/load boundary, not only at open.
        file.set_permissions(fs::Permissions::from_mode(0o640))?;
        if !matches!(
            store.require_body_entry(&name, &file),
            Err(CoreError::InvalidContract(_))
        ) {
            return Err("an opened upload body accepted group-readable mode drift".into());
        }
        file.set_permissions(fs::Permissions::from_mode(0o600))?;
        store.require_body_entry(&name, &file)?;

        let alias = parent.path().join("body-alias");
        fs::hard_link(store.path(identity)?, &alias)?;
        if !matches!(
            store.require_body_entry(&name, &file),
            Err(CoreError::InvalidContract(_))
        ) {
            return Err("an opened upload body accepted a new hardlink".into());
        }
        if fs::read(&alias)? != [7] || fs::read(store.path(identity)?)? != [7] {
            return Err("body authentication changed either linked body".into());
        }
        Ok(())
    }

    mod admitted {
        use super::*;
        use quanta_index_core::publication_admission::{
            PublicationControlFileMutation, PublicationControlOpenTicket,
        };
        use std::os::unix::fs::OpenOptionsExt as _;
        use std::sync::atomic::{AtomicBool, Ordering};

        struct RecordingAdmission {
            root: PathBuf,
            calls: Arc<Mutex<Vec<&'static str>>>,
            refuse_write: AtomicBool,
            refuse_read: Arc<AtomicBool>,
        }

        impl RecordingAdmission {
            fn new(root: PathBuf) -> Arc<Self> {
                Arc::new(Self {
                    root,
                    calls: Arc::new(Mutex::new(Vec::new())),
                    refuse_write: AtomicBool::new(false),
                    refuse_read: Arc::new(AtomicBool::new(false)),
                })
            }
            fn record(&self, call: &'static str) -> Result<(), CoreError> {
                self.calls.lock().map_err(storage)?.push(call);
                Ok(())
            }
            fn count(&self, call: &'static str) -> Result<usize, CoreError> {
                Ok(self
                    .calls
                    .lock()
                    .map_err(storage)?
                    .iter()
                    .filter(|observed| **observed == call)
                    .count())
            }
            fn require_path(&self, path: &Path) -> Result<(), CoreError> {
                if path.parent() == Some(self.root.as_path()) {
                    Ok(())
                } else {
                    Err(invalid("fixture received a foreign upload body"))
                }
            }
        }

        fn record_call(
            calls: &Mutex<Vec<&'static str>>,
            call: &'static str,
        ) -> Result<(), CoreError> {
            calls.lock().map_err(storage)?.push(call);
            Ok(())
        }
        struct RecordingTicket {
            calls: Arc<Mutex<Vec<&'static str>>>,
            refuse_read: Arc<AtomicBool>,
        }
        struct RecordingReader {
            file: File,
            calls: Arc<Mutex<Vec<&'static str>>>,
            refuse_read: Arc<AtomicBool>,
        }
        struct RecordingWriter {
            file: File,
            target_end: u64,
            calls: Arc<Mutex<Vec<&'static str>>>,
            temporary: Option<PathBuf>,
            target: PathBuf,
        }

        impl Read for RecordingReader {
            fn read(&mut self, bytes: &mut [u8]) -> io::Result<usize> {
                if self.refuse_read.load(Ordering::SeqCst) {
                    return Err(io::Error::other(CoreError::Typed {
                        code: SearchPlaneErrorCodeV2::CatalogBusy,
                        message: "fixture admitted reader refusal".into(),
                    }));
                }
                self.file.read(bytes)
            }
        }
        impl Seek for RecordingReader {
            fn seek(&mut self, position: SeekFrom) -> io::Result<u64> {
                self.file.seek(position)
            }
        }
        impl PublicationControlReader for RecordingReader {
            fn metadata(&self) -> Result<fs::Metadata, CoreError> {
                self.file.metadata().map_err(storage)
            }
            fn sync_posix(&self) -> Result<(), CoreError> {
                self.file.sync_all().map_err(storage)
            }
            fn lock_exclusive(&self) -> Result<(), CoreError> {
                Err(invalid("upload reader is not a gate"))
            }
            fn try_lock_exclusive(&self) -> Result<bool, CoreError> {
                Err(invalid("upload reader is not a gate"))
            }
            fn retire(self: Box<Self>) -> Result<(), CoreError> {
                record_call(&self.calls, "reader_retire")?;
                drop(self.file);
                Ok(())
            }
        }
        impl PublicationControlOpenTicket for RecordingTicket {
            fn opened_owned(
                self: Box<Self>,
                file: File,
            ) -> Result<Box<dyn PublicationControlReader>, CoreError> {
                record_call(&self.calls, "reader_opened")?;
                Ok(Box::new(RecordingReader {
                    file,
                    calls: self.calls.clone(),
                    refuse_read: self.refuse_read.clone(),
                }))
            }
        }
        impl Read for RecordingWriter {
            fn read(&mut self, bytes: &mut [u8]) -> io::Result<usize> {
                self.file.read(bytes)
            }
        }
        impl Write for RecordingWriter {
            fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
                self.file.write(bytes)
            }
            fn flush(&mut self) -> io::Result<()> {
                self.file.flush()
            }
        }
        impl Seek for RecordingWriter {
            fn seek(&mut self, position: SeekFrom) -> io::Result<u64> {
                self.file.seek(position)
            }
        }
        impl PublicationControlFileMutation for RecordingWriter {
            fn metadata(&self) -> Result<fs::Metadata, CoreError> {
                self.file.metadata().map_err(storage)
            }
            fn set_len(&mut self, length: u64) -> Result<(), CoreError> {
                self.file.set_len(length).map_err(storage)
            }
            fn sync_and_settle(self: Box<Self>) -> Result<(), CoreError> {
                if self.file.metadata().map_err(storage)?.len() != self.target_end {
                    return Err(invalid("fixture writer did not reach reserved end"));
                }
                self.file.sync_all().map_err(storage)?;
                if let Some(temporary) = &self.temporary {
                    if self.target.exists() {
                        return Err(invalid("fixture create target unexpectedly exists"));
                    }
                    fs::rename(temporary, &self.target).map_err(storage)?;
                    let parent = self
                        .target
                        .parent()
                        .ok_or_else(|| invalid("fixture target has no parent"))?;
                    File::open(parent)
                        .and_then(|directory| directory.sync_all())
                        .map_err(storage)?;
                }
                record_call(&self.calls, "writer_settled")?;
                drop(self.file);
                Ok(())
            }
            fn retire_preserving_pending(self: Box<Self>) -> Result<(), CoreError> {
                if let Some(temporary) = &self.temporary {
                    fs::remove_file(temporary).map_err(storage)?;
                }
                record_call(&self.calls, "writer_retired_pending")?;
                drop(self.file);
                Ok(())
            }
        }
        impl SourcePublicationUploadFileAdmissionPort for RecordingAdmission {
            fn validate_root_and_reconcile(&self, root: &Path) -> Result<(), CoreError> {
                if root != self.root {
                    return Err(invalid("fixture upload root mismatch"));
                }
                self.record("validate")
            }
            fn begin_write(
                &self,
                path: &Path,
                declared_body_bytes: u64,
                expected_length: Option<u64>,
                target_end: u64,
            ) -> Result<Box<dyn PublicationControlFileMutation>, CoreError> {
                self.require_path(path)?;
                self.record("begin_write")?;
                if self.refuse_write.load(Ordering::SeqCst) {
                    return Err(busy("fixture physical admission refusal"));
                }
                if target_end > declared_body_bytes {
                    return Err(invalid("fixture target exceeds declared body"));
                }
                let (file, temporary) = if let Some(length) = expected_length {
                    let file = fs::OpenOptions::new()
                        .read(true)
                        .write(true)
                        .open(path)
                        .map_err(storage)?;
                    if file.metadata().map_err(storage)?.len() != length {
                        return Err(invalid("fixture existing width mismatch"));
                    }
                    (file, None)
                } else {
                    let temporary = self.root.with_extension("fixture-stage");
                    let file = fs::OpenOptions::new()
                        .read(true)
                        .write(true)
                        .create_new(true)
                        .mode(0o600)
                        .open(&temporary)
                        .map_err(storage)?;
                    (file, Some(temporary))
                };
                Ok(Box::new(RecordingWriter {
                    file,
                    target_end,
                    calls: self.calls.clone(),
                    temporary,
                    target: path.to_owned(),
                }))
            }
            fn begin_reader_open(
                &self,
                path: &Path,
            ) -> Result<Box<dyn PublicationControlOpenTicket>, CoreError> {
                self.require_path(path)?;
                self.record("begin_reader")?;
                Ok(Box::new(RecordingTicket {
                    calls: self.calls.clone(),
                    refuse_read: self.refuse_read.clone(),
                }))
            }
            fn delete_file_and_sync(
                &self,
                path: &Path,
                inode: InodeKey,
                length: u64,
            ) -> Result<(), CoreError> {
                self.require_path(path)?;
                let meta = fs::symlink_metadata(path).map_err(storage)?;
                if (meta.dev(), meta.ino(), meta.len()) != (inode.device, inode.inode, length) {
                    return Err(invalid("fixture deletion witness mismatch"));
                }
                fs::remove_file(path).map_err(storage)?;
                File::open(&self.root)
                    .and_then(|directory| directory.sync_all())
                    .map_err(storage)?;
                self.record("delete")
            }
        }

        #[test]
        fn qualified_upload_uses_owned_growth_read_retry_and_delete() -> TestResult {
            let parent = tempfile::tempdir()?;
            let root = parent.path().join("uploads");
            let admission = RecordingAdmission::new(root);
            let store = SourcePublicationUploadStore::open_at_with_file_admission(
                File::open(parent.path())?,
                parent.path(),
                OsStr::new("uploads"),
                SOURCE_PUBLICATION_UPLOAD_MAX_BYTES,
                admission.clone(),
            )?;
            let mut payload = batch();
            payload.bundle_payload = None;
            let mut bytes = Vec::new();
            ciborium::into_writer(&payload, &mut bytes)?;
            let identity = SourcePublicationUploadIdentity {
                body_sha256: Sha256::digest(&bytes).into(),
                body_bytes: u64::try_from(bytes.len())?,
            };
            let split = bytes.len().checked_div(2).ok_or("invalid part split")?;
            let first = SourcePublicationUploadPart {
                identity,
                offset: 0,
                bytes: bytes.get(..split).ok_or("first part is absent")?.to_vec(),
            };
            let second = SourcePublicationUploadPart {
                identity,
                offset: u64::try_from(split)?,
                bytes: bytes.get(split..).ok_or("second part is absent")?.to_vec(),
            };
            if store.stage(&first, &budget())?.next_offset != u64::try_from(split)?
                || store.stage(&first, &budget())?.next_offset != u64::try_from(split)?
                || store.stage(&second, &budget())?.next_offset != identity.body_bytes
                || store.load(identity, &budget())? != payload
            {
                return Err("qualified upload changed bytes or retry offset".into());
            }
            store.discard(identity)?;
            if store.path(identity)?.exists()
                || admission.count("begin_write")? != 2
                || admission.count("writer_settled")? != 2
                || admission.count("delete")? != 1
                || admission.count("begin_reader")? != admission.count("reader_retire")?
            {
                return Err("qualified upload did not settle exact physical operations".into());
            }
            Ok(())
        }

        #[test]
        fn qualified_write_refusal_precedes_body_creation() -> TestResult {
            let parent = tempfile::tempdir()?;
            let root = parent.path().join("uploads");
            let foreign = RecordingAdmission::new(parent.path().join("foreign"));
            if SourcePublicationUploadStore::open_at_with_file_admission(
                File::open(parent.path())?,
                parent.path(),
                OsStr::new("uploads"),
                SOURCE_PUBLICATION_UPLOAD_MAX_BYTES,
                foreign,
            )
            .is_ok()
                || root.exists()
            {
                return Err("foreign physical authority created the upload root".into());
            }
            let admission = RecordingAdmission::new(root);
            admission.refuse_write.store(true, Ordering::SeqCst);
            let store = SourcePublicationUploadStore::open_at_with_file_admission(
                File::open(parent.path())?,
                parent.path(),
                OsStr::new("uploads"),
                SOURCE_PUBLICATION_UPLOAD_MAX_BYTES,
                admission.clone(),
            )?;
            let identity = SourcePublicationUploadIdentity {
                body_sha256: [7; 32],
                body_bytes: 1,
            };
            let part = SourcePublicationUploadPart {
                identity,
                offset: 0,
                bytes: vec![7],
            };
            if !matches!(
                store.stage(&part, &budget()),
                Err(CoreError::Typed {
                    code: SearchPlaneErrorCodeV2::CatalogBusy,
                    ..
                })
            ) || store.path(identity)?.exists()
                || admission.count("begin_write")? != 1
            {
                return Err("qualified admission refusal created a body or lost its type".into());
            }
            Ok(())
        }

        #[test]
        fn qualified_read_failure_preserves_typed_cause_and_retires_original_reader() -> TestResult
        {
            let parent = tempfile::tempdir()?;
            let root = parent.path().join("uploads");
            let admission = RecordingAdmission::new(root);
            let store = SourcePublicationUploadStore::open_at_with_file_admission(
                File::open(parent.path())?,
                parent.path(),
                OsStr::new("uploads"),
                SOURCE_PUBLICATION_UPLOAD_MAX_BYTES,
                admission.clone(),
            )?;
            let identity = SourcePublicationUploadIdentity {
                body_sha256: [9; 32],
                body_bytes: 1,
            };
            let part = SourcePublicationUploadPart {
                identity,
                offset: 0,
                bytes: vec![9],
            };
            let _ack = store.stage(&part, &budget())?;
            admission.refuse_read.store(true, Ordering::SeqCst);
            if !matches!(
                store.load(identity, &budget()),
                Err(CoreError::Typed {
                    code: SearchPlaneErrorCodeV2::CatalogBusy,
                    ..
                })
            ) || admission.count("begin_reader")? != admission.count("reader_retire")?
                || fs::read(store.path(identity)?)? != [9]
            {
                return Err(
                    "qualified read failure lost typed cause, original FD retirement, or body"
                        .into(),
                );
            }
            Ok(())
        }

        #[test]
        fn admitted_io_cause_keeps_typed_and_raw_errors_without_text_inference() -> TestResult {
            let typed = CoreError::Typed {
                code: SearchPlaneErrorCodeV2::CatalogBusy,
                message: "fixture refusal".into(),
            };
            let nested = io::Error::other(io::Error::other(typed));
            if !matches!(
                admitted_io_error("nested", &nested),
                CoreError::Typed {
                    code: SearchPlaneErrorCodeV2::CatalogBusy,
                    ..
                }
            ) {
                return Err("nested admitted error lost its typed cause".into());
            }
            let raw = io::Error::from_raw_os_error(28);
            if !matches!(
                admitted_io_error("raw", &raw),
                CoreError::StorageIo {
                    raw_os_error: Some(28),
                    ..
                }
            ) {
                return Err("raw filesystem failure lost its errno".into());
            }
            let lookalike = io::Error::other("typed failure CatalogBusy: fixture refusal");
            if matches!(
                admitted_io_error("text", &lookalike),
                CoreError::Typed { .. }
            ) {
                return Err("error message was promoted into a typed cause".into());
            }
            Ok(())
        }
    }
}
