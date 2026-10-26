//! Physical `RepoMap` projections on the publication ledger.

use std::ffi::{OsStr, OsString};
use std::fmt::Write as _;
use std::fs::{File, Metadata};
use std::io::{self, Read, Seek, SeekFrom, Write as _};
use std::os::fd::AsFd as _;
use std::os::unix::ffi::{OsStrExt as _, OsStringExt as _};
use std::os::unix::fs::MetadataExt as _;
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;

use quanta_index_contract::SearchPlaneErrorCodeV2;
use quanta_index_core::publication_admission::{
    PublicationObjectReader, PublicationOutputClass, PublicationOutputPermit,
    PublicationOutputReservation,
};
use quanta_index_core::publication_capacity::InodeKey;
use quanta_index_core::{
    CoreError, RepoMapImmutablePutOutcome, RepoMapPhysicalFileClass, RepoMapPhysicalFilePort,
};
use quanta_index_repomap::{
    CandidateObjectAddressV1, QuarantineIncidentAddressV1, QuarantinePayloadAddressV1,
};
use quanta_index_searchd::app::runtime::StateRootDirectoryV1;
use rusqlite::ffi::quanta_capacity_vfs::close_owned_file;
use rustix::fs::{
    AtFlags, Dir, Mode, OFlags, RenameFlags, fstat, openat, renameat_with, statat, unlinkat,
};

use crate::publication_file_capacity::{PublicationFileCapacity, ReaderLease};
use crate::publication_file_deletions::PublicationFileDeletions;
use crate::publication_sidecar::recoverable_sidecar_output_permit;

const REPOMAP_DIRECTORY: &str = "repo-map";
const ROOT_UUID_NAME: &str = "root-uuid.bin";
const TEMP_PREFIX: &str = ".qiv-repomap-";

fn contract(message: &str) -> CoreError {
    CoreError::InvalidContract(format!("RepoMap physical file admission: {message}"))
}

fn storage_io(context: &str, error: impl Into<io::Error>) -> CoreError {
    let error = error.into();
    admitted_io_error(
        &format!("RepoMap physical file admission {context}"),
        &error,
    )
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

fn budget(message: &str) -> CoreError {
    CoreError::Typed {
        code: SearchPlaneErrorCodeV2::IngestResourceBudgetExceeded,
        message: format!("RepoMap physical file admission: {message}"),
    }
}

fn io_error(error: CoreError) -> io::Error {
    match error {
        absent @ CoreError::NotFound(_) => io::Error::new(io::ErrorKind::NotFound, absent),
        storage @ CoreError::StorageIo { kind, .. } => io::Error::new(kind, storage),
        other @ (CoreError::InvalidContract(_)
        | CoreError::Typed { .. }
        | CoreError::NotReady(_)
        | CoreError::NotImplemented(_)
        | CoreError::Storage(_)) => io::Error::other(other),
    }
}

fn append_custody(mut primary: CoreError, context: &str, additional: &CoreError) -> CoreError {
    let message = match &mut primary {
        CoreError::InvalidContract(message)
        | CoreError::NotReady(message)
        | CoreError::NotImplemented(message)
        | CoreError::NotFound(message)
        | CoreError::Storage(message)
        | CoreError::StorageIo { message, .. }
        | CoreError::Typed { message, .. } => message,
    };
    let _formatted = write!(message, "; {context}: {additional}");
    primary
}

fn lowercase_hex_pair(component: &[u8]) -> bool {
    component.len() == 2
        && component
            .iter()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(byte))
}

/// Service-owned `RepoMap` file authority.
///
/// `deletions` owns the same bounded pending slot. The held root prevents use
/// after lease handoff.
pub struct CanonicalRepoMapFileAdmission {
    binder: Arc<PublicationFileCapacity>,
    deletions: Arc<PublicationFileDeletions>,
    held_root: StateRootDirectoryV1,
    root: PathBuf,
    max_control_file_bytes: u64,
    max_candidate_file_bytes: u64,
    max_boot_temp_scan_entries: usize,
}

impl CanonicalRepoMapFileAdmission {
    pub fn new(
        deletions: Arc<PublicationFileDeletions>,
        held_root: StateRootDirectoryV1,
        max_control_file_bytes: u64,
        max_candidate_file_bytes: u64,
        max_boot_temp_scan_entries: usize,
    ) -> Result<Self, CoreError> {
        if max_control_file_bytes == 0
            || max_candidate_file_bytes == 0
            || max_boot_temp_scan_entries == 0
        {
            return Err(contract("all file and boot-scan maxima must be positive"));
        }
        let binder = deletions.binder();
        binder.validate_current_state_root()?;
        held_root.require_current_v1()?;
        let pinned = fstat(&held_root).map_err(|error| storage_io("stat held root", error))?;
        let named = statat(
            rustix::fs::CWD,
            binder.state_root(),
            AtFlags::SYMLINK_NOFOLLOW,
        )
        .map_err(|error| storage_io("stat binder root", error))?;
        if (pinned.st_dev, pinned.st_ino) != (named.st_dev, named.st_ino) {
            return Err(contract("held root differs from binder root"));
        }
        let admission = Self {
            root: binder.state_root().join(REPOMAP_DIRECTORY),
            binder,
            deletions,
            held_root,
            max_control_file_bytes,
            max_candidate_file_bytes,
            max_boot_temp_scan_entries,
        };
        admission.reconcile_boot_temporaries()?;
        Ok(admission)
    }

    fn validate_root(&self, root: &Path) -> Result<(), CoreError> {
        if root != self.root {
            return Err(contract("path is not under the canonical RepoMap root"));
        }
        self.binder.validate_current_state_root()?;
        self.held_root.require_current_v1()?;
        let pinned = fstat(&self.held_root).map_err(|error| storage_io("stat held root", error))?;
        let named = statat(
            rustix::fs::CWD,
            self.binder.state_root(),
            AtFlags::SYMLINK_NOFOLLOW,
        )
        .map_err(|error| storage_io("stat current root", error))?;
        if (pinned.st_dev, pinned.st_ino) != (named.st_dev, named.st_ino) {
            return Err(contract("held root incarnation changed"));
        }
        Ok(())
    }

    /// Boot inventory has already charged these files. Verify a complete,
    /// bounded root scan before unlinking any exact orphan through the shared
    /// deletion owner. Live creates are reconciled first and refuse quiescence.
    fn reconcile_boot_temporaries(&self) -> Result<(), CoreError> {
        self.validate_root(&self.root)?;
        self.deletions.reconcile_retired_control_creates()?;
        if !self.binder.is_quiescent()? {
            return Err(contract("boot temporary scan requires a quiescent binder"));
        }
        let mut names = Vec::new();
        let mut seen = 0_usize;
        let mut directory =
            Dir::read_from(&self.held_root).map_err(|error| storage_io("scan held root", error))?;
        while let Some(entry) = directory.read() {
            let entry = entry.map_err(|error| storage_io("read held root entry", error))?;
            let raw = entry.file_name().to_bytes();
            if raw == b"." || raw == b".." {
                continue;
            }
            seen = seen
                .checked_add(1)
                .ok_or_else(|| budget("boot scan entry count overflow"))?;
            if seen > self.max_boot_temp_scan_entries {
                return Err(budget("boot scan exceeds configured entry maximum"));
            }
            let name = OsString::from_vec(raw.to_vec());
            let Some(text) = name.to_str() else {
                if raw.starts_with(TEMP_PREFIX.as_bytes()) {
                    return Err(contract("non-UTF-8 RepoMap staging name"));
                }
                continue;
            };
            let Some(nonce) = text.strip_prefix(TEMP_PREFIX) else {
                continue;
            };
            let Some(hex) = nonce.strip_suffix(".tmp") else {
                return Err(contract("malformed RepoMap staging name"));
            };
            if hex.len() != 32
                || !hex
                    .bytes()
                    .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
            {
                return Err(contract("malformed RepoMap staging nonce"));
            }
            names.push(name);
        }
        // Validate every candidate before removing any of them. A foreign
        // symlink, alias or replacement cannot turn partial scanning into a
        // refund of an inode whose physical identity was never witnessed.
        let mut orphaned = Vec::with_capacity(names.len());
        for name in names {
            let path = self.binder.state_root().join(&name);
            let opened = File::from(
                openat(
                    &self.held_root,
                    Path::new(&name),
                    OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::NONBLOCK | OFlags::CLOEXEC,
                    Mode::empty(),
                )
                .map_err(|error| storage_io("open boot temporary", error))?,
            );
            let witnessed = (|| {
                let actual = opened
                    .metadata()
                    .map_err(|error| storage_io("stat boot temporary FD", error))?;
                let entry_stat =
                    statat(&self.held_root, Path::new(&name), AtFlags::SYMLINK_NOFOLLOW)
                        .map_err(|error| storage_io("stat boot temporary name", error))?;
                if !actual.is_file()
                    || actual.nlink() != 1
                    || actual.uid() != rustix::process::geteuid().as_raw()
                    || actual.mode() & 0o777 != 0o600
                    || actual.dev()
                        != crate::publication_inventory::checked_device_id(entry_stat.st_dev)
                            .map_err(|_error| contract("boot temporary device does not fit u64"))?
                    || actual.ino() != entry_stat.st_ino
                    || actual.len()
                        != u64::try_from(entry_stat.st_size)
                            .map_err(|_error| contract("boot temporary width does not fit u64"))?
                {
                    return Err(contract("boot temporary is not the private named inode"));
                }
                let inode = InodeKey {
                    device: actual.dev(),
                    inode: actual.ino(),
                };
                let seeded = self
                    .binder
                    .path_observation(&path)?
                    .ok_or_else(|| contract("boot temporary is absent from sealed inventory"))?;
                if seeded.inode != inode || seeded.bytes != actual.len() {
                    return Err(contract("boot temporary differs from seeded inode"));
                }
                Ok((inode, actual.len()))
            })();
            let closed = close_owned_file(opened)
                .map_err(|error| storage_io("close boot temporary observer", error));
            if closed.is_err() {
                self.binder.mark_close_unwitnessed();
            }
            let (inode, bytes) = match (witnessed, closed) {
                (Ok(observed), Ok(())) => observed,
                (Err(primary), Ok(())) => return Err(primary),
                (Ok(_), Err(close)) => return Err(close),
                (Err(primary), Err(close)) => {
                    return Err(append_custody(primary, "observer retirement", &close));
                }
            };
            orphaned.push((path, inode, bytes));
        }
        for (path, inode, bytes) in orphaned {
            self.validate_root(&self.root)?;
            if !self.binder.is_quiescent()? {
                return Err(contract("boot temporary cleanup met a live mutation"));
            }
            self.deletions
                .delete_owned_file_and_sync(&path, inode, bytes)?;
        }
        Ok(())
    }

    fn relative_components(&self, path: &Path) -> Result<Vec<OsString>, CoreError> {
        if !path.is_absolute() {
            return Err(contract("path is not absolute"));
        }
        let relative = path
            .strip_prefix(&self.root)
            .map_err(|_error| contract("path escapes RepoMap root"))?;
        let mut components = Vec::new();
        let mut canonical = self.root.clone();
        for component in relative.components() {
            let Component::Normal(name) = component else {
                return Err(contract("path has a non-normal component"));
            };
            components.push(name.to_owned());
            canonical.push(name);
        }
        if canonical.as_os_str() != path.as_os_str() {
            return Err(contract("path differs from canonical RepoMap path"));
        }
        Ok(components)
    }

    fn file_class(&self, path: &Path) -> Result<RepoMapPhysicalFileClass, CoreError> {
        let components = self.relative_components(path)?;
        if components.len() == 1
            && components
                .first()
                .is_some_and(|name| name.as_os_str() == OsStr::new(ROOT_UUID_NAME))
        {
            return Ok(RepoMapPhysicalFileClass::RootUuid);
        }
        let bytes = components
            .iter()
            .map(|component| component.as_bytes())
            .collect::<Vec<_>>();
        if CandidateObjectAddressV1::parse_components(&bytes).is_ok() {
            return Ok(RepoMapPhysicalFileClass::Candidate);
        }
        if QuarantineIncidentAddressV1::parse_components(&bytes).is_ok() {
            return Ok(RepoMapPhysicalFileClass::Incident);
        }
        if QuarantinePayloadAddressV1::parse_components(&bytes).is_ok() {
            return Ok(RepoMapPhysicalFileClass::Payload);
        }
        Err(contract("file path is not a canonical RepoMap projection"))
    }

    fn file_bound(&self, class: RepoMapPhysicalFileClass) -> u64 {
        match class {
            RepoMapPhysicalFileClass::Candidate => self.max_candidate_file_bytes,
            RepoMapPhysicalFileClass::RootUuid
            | RepoMapPhysicalFileClass::Incident
            | RepoMapPhysicalFileClass::Payload => self.max_control_file_bytes,
        }
    }

    fn valid_directory(&self, path: &Path) -> Result<(), CoreError> {
        let components = self.relative_components(path)?;
        let bytes = components
            .iter()
            .map(|component| component.as_bytes())
            .collect::<Vec<_>>();
        if bytes.is_empty() {
            return Ok(());
        }
        let suffix: &[&[u8]] = if bytes.first().is_some_and(|name| *name == &b"objects"[..]) {
            bytes
                .get(1..)
                .ok_or_else(|| contract("objects prefix absent"))?
        } else if bytes
            .first()
            .is_some_and(|name| *name == &b"quarantine"[..])
        {
            if bytes.len() == 1 {
                return Ok(());
            }
            if !bytes
                .get(1)
                .is_some_and(|name| *name == &b"incidents"[..] || *name == &b"payloads"[..])
            {
                return Err(contract("directory is not a RepoMap projection prefix"));
            }
            bytes
                .get(2..)
                .ok_or_else(|| contract("quarantine prefix absent"))?
        } else {
            return Err(contract("directory is not a RepoMap projection prefix"));
        };
        if suffix.len() > 3
            || suffix.first().is_some_and(|name| *name != &b"sha256"[..])
            || suffix.get(1).is_some_and(|name| !lowercase_hex_pair(name))
            || suffix.get(2).is_some_and(|name| !lowercase_hex_pair(name))
        {
            return Err(contract("directory fanout is not canonical"));
        }
        Ok(())
    }

    /// Walk from the held root with `O_NOFOLLOW` at every component. Close
    /// every temporary directory FD before returning the callback result.
    fn with_parent<T>(
        &self,
        path: &Path,
        action: impl FnOnce(std::os::fd::BorrowedFd<'_>, &OsStr) -> Result<T, CoreError>,
    ) -> Result<T, CoreError> {
        let relative = path
            .strip_prefix(self.binder.state_root())
            .map_err(|_error| contract("anchored path escapes held root"))?;
        let mut components = relative.components().collect::<Vec<_>>();
        let Some(Component::Normal(leaf)) = components.pop() else {
            return Err(contract("anchored path lacks a normal leaf"));
        };
        let mut parents = Vec::<File>::new();
        let result = (|| {
            for component in components {
                let Component::Normal(name) = component else {
                    return Err(contract("anchored parent has a non-normal component"));
                };
                let parent = parents
                    .last()
                    .map_or_else(|| self.held_root.as_fd(), File::as_fd);
                let next = File::from(
                    openat(
                        parent,
                        Path::new(name),
                        OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
                        Mode::empty(),
                    )
                    .map_err(|error| storage_io("open nofollow parent", error))?,
                );
                parents.push(next);
                let metadata = parents
                    .last()
                    .ok_or_else(|| contract("opened nofollow parent disappeared"))?
                    .metadata()
                    .map_err(|error| storage_io("stat nofollow parent", error))?;
                if !metadata.is_dir()
                    || metadata.uid() != rustix::process::geteuid().as_raw()
                    || metadata.mode() & 0o077 != 0
                {
                    return Err(contract("RepoMap parent is not private to the owner"));
                }
            }
            let parent = parents
                .last()
                .map_or_else(|| self.held_root.as_fd(), File::as_fd);
            action(parent, leaf)
        })();
        let mut close_failure = None;
        for parent in parents.into_iter().rev() {
            if let Err(error) = close_owned_file(parent) {
                self.binder.mark_close_unwitnessed();
                close_failure = Some(storage_io("close nofollow parent", error));
            }
        }
        match (result, close_failure) {
            (Ok(value), None) => Ok(value),
            (Err(primary), None) => Err(primary),
            (Ok(_), Some(close)) => Err(close),
            (Err(primary), Some(close)) => {
                Err(append_custody(primary, "parent retirement", &close))
            }
        }
    }

    fn private_temporary(&self) -> Result<PathBuf, CoreError> {
        let mut random = [0_u8; 16];
        let mut source = File::open("/dev/urandom")
            .map_err(|error| storage_io("open temp nonce source", error))?;
        let read = source
            .read_exact(&mut random)
            .map_err(|error| storage_io("read temp nonce", error));
        let close =
            close_owned_file(source).map_err(|error| storage_io("close temp nonce source", error));
        match (read, close) {
            (Ok(()), Ok(())) => {}
            (Err(primary), Ok(())) | (Ok(()), Err(primary)) => return Err(primary),
            (Err(primary), Err(close)) => {
                return Err(append_custody(primary, "nonce source retirement", &close));
            }
        }
        let mut hex = String::with_capacity(32);
        for byte in random {
            write!(&mut hex, "{byte:02x}")
                .map_err(|error| CoreError::Storage(format!("RepoMap temp nonce: {error}")))?;
        }
        Ok(self
            .binder
            .state_root()
            .join(format!("{TEMP_PREFIX}{hex}.tmp")))
    }

    fn close_writer_preserving(
        &self,
        permit: &mut dyn PublicationOutputPermit,
        writer: File,
        primary: CoreError,
    ) -> CoreError {
        match permit.retire_writer_preserving_pending(writer) {
            Ok(()) => primary,
            Err(close) => append_custody(primary, "writer retirement", &close),
        }
    }

    fn cancel_staging(
        &self,
        permit: &mut dyn PublicationOutputPermit,
        temporary: &Path,
        writer: File,
        primary: CoreError,
    ) -> CoreError {
        let cleanup = (|| {
            let metadata = writer
                .metadata()
                .map_err(|error| storage_io("stat staging writer", error))?;
            let leaf = temporary
                .file_name()
                .ok_or_else(|| contract("staging name absent"))?;
            let named = statat(&self.held_root, Path::new(leaf), AtFlags::SYMLINK_NOFOLLOW)
                .map_err(|error| storage_io("stat staging leaf", error))?;
            if metadata.dev()
                != crate::publication_inventory::checked_device_id(named.st_dev)
                    .map_err(|_error| contract("staging device does not fit u64"))?
                || metadata.ino() != named.st_ino
                || metadata.nlink() != 1
            {
                return Err(contract("staging pathname differs from original writer"));
            }
            unlinkat(&self.held_root, Path::new(leaf), AtFlags::empty())
                .map_err(|error| storage_io("unlink exact staging", error))?;
            rustix::fs::fsync(&self.held_root)
                .map_err(|error| storage_io("sync staging parent", error))
        })();
        match cleanup {
            Ok(()) => match permit.confirmed_safe_absent_after_writer_close(writer) {
                Ok(()) => primary,
                Err(error) => append_custody(primary, "staging settlement", &error),
            },
            Err(cleanup) => {
                let original = append_custody(primary, "staging cleanup", &cleanup);
                self.close_writer_preserving(permit, writer, original)
            }
        }
    }

    fn exact_replay(
        &self,
        path: &Path,
        bytes: &[u8],
    ) -> Result<Option<RepoMapImmutablePutOutcome>, CoreError> {
        let Some(mut reader) = self.open_reader(path)? else {
            return Ok(None);
        };
        let checked = (|| {
            let width = reader.metadata()?.len();
            if width
                != u64::try_from(bytes.len()).map_err(|error| {
                    CoreError::Storage(format!("RepoMap encoded length does not fit u64: {error}"))
                })?
            {
                return Err(contract("existing immutable projection has another width"));
            }
            let mut actual = Vec::new();
            let _read = (&mut *reader)
                .take(width.saturating_add(1))
                .read_to_end(&mut actual)
                .map_err(|error| storage_io("read exact replay", error))?;
            if actual != bytes {
                return Err(contract(
                    "existing immutable projection differs from replay",
                ));
            }
            Ok(RepoMapImmutablePutOutcome::ExactReplay)
        })();
        let retirement = reader.retire();
        match (checked, retirement) {
            (Ok(outcome), Ok(())) => Ok(Some(outcome)),
            (Err(primary), Ok(())) => Err(primary),
            (Ok(_), Err(retirement)) => Err(retirement),
            (Err(primary), Err(retirement)) => Err(append_custody(
                primary,
                "replay reader retirement",
                &retirement,
            )),
        }
    }
}

struct RepoMapReader(ReaderLease);

impl io::Read for RepoMapReader {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        let mut file = self.0.file().map_err(io_error)?;
        file.read(buffer)
    }
}

impl Seek for RepoMapReader {
    fn seek(&mut self, position: SeekFrom) -> io::Result<u64> {
        let mut file = self.0.file().map_err(io_error)?;
        file.seek(position)
    }
}

impl PublicationObjectReader for RepoMapReader {
    fn metadata(&self) -> Result<Metadata, CoreError> {
        self.0
            .file()?
            .metadata()
            .map_err(|error| storage_io("reader metadata", error))
    }

    fn sync_posix(&self) -> Result<(), CoreError> {
        rustix::fs::fsync(self.0.file()?).map_err(|error| storage_io("reader POSIX fsync", error))
    }

    fn retire(self: Box<Self>) -> Result<(), CoreError> {
        let Self(reader) = *self;
        reader.retire()
    }
}

impl RepoMapPhysicalFilePort for CanonicalRepoMapFileAdmission {
    fn validate_root_and_reconcile(&self, root: &Path) -> Result<(), CoreError> {
        self.validate_root(root)?;
        self.deletions.reconcile_retired_control_creates()
    }

    fn ensure_private_directory(&self, path: &Path) -> Result<(), CoreError> {
        self.validate_root(&self.root)?;
        self.valid_directory(path)?;
        self.binder
            .ensure_private_parent_directories(&path.join(".directory-probe"))?;
        self.with_parent(path, |parent, leaf| {
            let directory = File::from(
                openat(
                    parent,
                    Path::new(leaf),
                    OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
                    Mode::empty(),
                )
                .map_err(|error| storage_io("open private directory", error))?,
            );
            let result = (|| {
                let metadata = directory
                    .metadata()
                    .map_err(|error| storage_io("stat private directory", error))?;
                if !metadata.is_dir()
                    || metadata.uid() != rustix::process::geteuid().as_raw()
                    || metadata.mode() & 0o777 != 0o700
                {
                    return Err(contract("RepoMap directory is not owner-private 0700"));
                }
                Ok(())
            })();
            let closed = close_owned_file(directory)
                .map_err(|error| storage_io("close private directory", error));
            if closed.is_err() {
                self.binder.mark_close_unwitnessed();
            }
            match (result, closed) {
                (Ok(()), Ok(())) => Ok(()),
                (Err(primary), Ok(())) => Err(primary),
                (Ok(()), Err(close)) => Err(close),
                (Err(primary), Err(close)) => {
                    Err(append_custody(primary, "directory retirement", &close))
                }
            }
        })?;
        self.validate_root(&self.root)
    }

    fn put_immutable(
        &self,
        path: &Path,
        bytes: &[u8],
        class: RepoMapPhysicalFileClass,
    ) -> Result<RepoMapImmutablePutOutcome, CoreError> {
        self.validate_root_and_reconcile(&self.root)?;
        if self.file_class(path)? != class {
            return Err(contract("file class differs from canonical pathname"));
        }
        let width = u64::try_from(bytes.len())
            .map_err(|error| CoreError::Storage(format!("RepoMap byte width: {error}")))?;
        if width > self.file_bound(class) {
            return Err(budget("encoded file exceeds configured class maximum"));
        }
        if let Some(replay) = self.exact_replay(path, bytes)? {
            return Ok(replay);
        }
        self.binder.ensure_private_parent_directories(path)?;
        self.with_parent(path, |_parent, _leaf| Ok(()))?;
        let provisional = width.max(1);
        let parent = self.binder.reserve_publication_allowance(provisional, 1)?;
        let output_class = match class {
            RepoMapPhysicalFileClass::Candidate => PublicationOutputClass::Source,
            RepoMapPhysicalFileClass::RootUuid
            | RepoMapPhysicalFileClass::Incident
            | RepoMapPhysicalFileClass::Payload => PublicationOutputClass::Control,
        };
        let request = PublicationOutputReservation::new(output_class, provisional, 1)?;
        let mut permit =
            recoverable_sidecar_output_permit(Arc::clone(&self.deletions), parent, request)?;
        let temporary = self.private_temporary()?;
        permit.bind_output(&temporary, path)?;
        permit.creation_started()?;
        let name = temporary
            .file_name()
            .ok_or_else(|| contract("private staging name absent"))?;
        let writer = match openat(
            &self.held_root,
            Path::new(name),
            OFlags::RDWR | OFlags::CREATE | OFlags::EXCL | OFlags::NOFOLLOW | OFlags::CLOEXEC,
            Mode::from_raw_mode(0o600),
        ) {
            Ok(opened) => File::from(opened),
            Err(error) => {
                let primary = storage_io("create private staging", error);
                return match statat(&self.held_root, Path::new(name), AtFlags::SYMLINK_NOFOLLOW) {
                    Err(rustix::io::Errno::NOENT) => {
                        let proof = rustix::fs::fsync(&self.held_root)
                            .map_err(|error| storage_io("sync uncreated staging parent", error))
                            .and_then(|()| permit.confirmed_uncreated_absent_and_synced());
                        match proof {
                            Ok(()) => Err(primary),
                            Err(error) => {
                                Err(append_custody(primary, "uncreated staging proof", &error))
                            }
                        }
                    }
                    Ok(_) => Err(primary),
                    Err(error) => Err(append_custody(
                        primary,
                        "stat failed create",
                        &storage_io("stat failed staging", error),
                    )),
                };
            }
        };
        if let Err(primary) = permit.created(&writer, &temporary) {
            return Err(match permit.cancel_rejected_created(writer) {
                Ok(()) => primary,
                Err(cleanup) => append_custody(primary, "rejected birth cleanup", &cleanup),
            });
        }
        let mut writer = writer;
        let write_result = writer
            .write_all(bytes)
            .and_then(|()| writer.sync_all())
            .map_err(|error| storage_io("write and sync staged projection", error))
            .and_then(|()| permit.before_replace());
        if let Err(primary) = write_result {
            return Err(self.cancel_staging(permit.as_mut(), &temporary, writer, primary));
        }
        let mut renamed_by_kernel = false;
        let renamed = self.with_parent(path, |parent, leaf| {
            renameat_with(
                &self.held_root,
                Path::new(name),
                parent,
                Path::new(leaf),
                RenameFlags::NOREPLACE,
            )
            .map_err(|error| storage_io("publish without replacement", error))?;
            renamed_by_kernel = true;
            Ok(())
        });
        if let Err(primary) = renamed {
            // An actual successful rename followed by a failed parent close
            // needs pending recovery. A failed NOREPLACE syscall left the
            // original staging inode in place for checked cleanup.
            return Err(if renamed_by_kernel {
                self.close_writer_preserving(permit.as_mut(), writer, primary)
            } else {
                self.cancel_staging(permit.as_mut(), &temporary, writer, primary)
            });
        }
        let synced = self.with_parent(path, |parent, _leaf| {
            rustix::fs::fsync(parent).map_err(|error| storage_io("sync projection parent", error))
        });
        if let Err(primary) = synced.and_then(|()| {
            rustix::fs::fsync(&self.held_root)
                .map_err(|error| storage_io("sync staging parent", error))
        }) {
            return Err(self.close_writer_preserving(permit.as_mut(), writer, primary));
        }
        permit.committed(writer, width)?;
        Ok(RepoMapImmutablePutOutcome::Created)
    }

    fn open_reader(
        &self,
        path: &Path,
    ) -> Result<Option<Box<dyn PublicationObjectReader>>, CoreError> {
        self.validate_root_and_reconcile(&self.root)?;
        let class = self.file_class(path)?;
        let guard = match self.binder.begin_reader_open(path) {
            Ok(guard) => guard,
            Err(CoreError::NotFound(_)) => return Ok(None),
            Err(error) => return Err(error),
        };
        let reader = self.with_parent(path, |parent, leaf| {
            let file = File::from(
                openat(
                    parent,
                    Path::new(leaf),
                    OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::NONBLOCK | OFlags::CLOEXEC,
                    Mode::empty(),
                )
                .map_err(|error| storage_io("open original reader", error))?,
            );
            guard.opened_owned(file)
        })?;
        let checked = (|| {
            let metadata = reader
                .file()?
                .metadata()
                .map_err(|error| storage_io("stat original reader", error))?;
            if !metadata.is_file()
                || metadata.len() > self.file_bound(class)
                || metadata.nlink() != 1
                || metadata.uid() != rustix::process::geteuid().as_raw()
                || metadata.mode() & 0o777 != 0o600
            {
                return Err(contract(
                    "reader file exceeds bound or differs from private inode",
                ));
            }
            Ok(())
        })();
        if let Err(primary) = checked {
            return Err(match reader.retire() {
                Ok(()) => primary,
                Err(retirement) => append_custody(primary, "reader retirement", &retirement),
            });
        }
        Ok(Some(Box::new(RepoMapReader(reader))))
    }

    fn delete_file_and_sync(
        &self,
        path: &Path,
        expected_inode: InodeKey,
        expected_length: u64,
    ) -> Result<(), CoreError> {
        self.validate_root_and_reconcile(&self.root)?;
        let class = self.file_class(path)?;
        if expected_length > self.file_bound(class) {
            return Err(budget("delete witness exceeds configured file maximum"));
        }
        self.deletions
            .delete_owned_file_and_sync(path, expected_inode, expected_length)
    }
}

#[cfg(test)]
mod tests;

#[cfg(test)]
mod error_tests {
    use super::*;

    #[test]
    fn admitted_storage_mapper_preserves_nested_typed_refusal() -> Result<(), CoreError> {
        let original = CoreError::Typed {
            code: SearchPlaneErrorCodeV2::IngestResourceBudgetExceeded,
            message: "fixed original admission refusal".into(),
        };
        let inner = io::Error::other(original);
        let outer = io::Error::other(inner);
        match storage_io("read", outer) {
            CoreError::Typed {
                code: SearchPlaneErrorCodeV2::IngestResourceBudgetExceeded,
                message,
            } if message == "fixed original admission refusal" => Ok(()),
            other @ (CoreError::InvalidContract(_)
            | CoreError::Typed { .. }
            | CoreError::NotReady(_)
            | CoreError::NotImplemented(_)
            | CoreError::NotFound(_)
            | CoreError::Storage(_)
            | CoreError::StorageIo { .. }) => {
                Err(contract(&format!("nested refusal was replaced: {other}")))
            }
        }
    }

    #[test]
    fn admitted_storage_mapper_retains_actual_missing_file_cause() -> Result<(), CoreError> {
        match storage_io("open", io::Error::from_raw_os_error(2)) {
            CoreError::StorageIo {
                kind: io::ErrorKind::NotFound,
                raw_os_error: Some(2),
                ..
            } => Ok(()),
            other @ (CoreError::InvalidContract(_)
            | CoreError::Typed { .. }
            | CoreError::NotReady(_)
            | CoreError::NotImplemented(_)
            | CoreError::NotFound(_)
            | CoreError::Storage(_)
            | CoreError::StorageIo { .. }) => {
                Err(contract(&format!("filesystem cause was replaced: {other}")))
            }
        }
    }
}
