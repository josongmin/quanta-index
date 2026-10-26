//! Pre-event source upload bodies on the composition-owned publication ledger.

use std::collections::BTreeMap;
use std::ffi::{OsStr, OsString};
use std::fs::{File, Metadata};
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::os::unix::ffi::OsStringExt as _;
use std::os::unix::fs::MetadataExt as _;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use quanta_index_contract::SearchPlaneErrorCodeV2;
use quanta_index_core::publication_admission::{
    PublicationControlFileMutation, PublicationControlOpenTicket, PublicationOutputClass,
    PublicationOutputPermit, PublicationOutputReservation,
};
use quanta_index_core::publication_capacity::{InodeKey, PublicationAllowance};
use quanta_index_core::{CoreError, SourcePublicationUploadFileAdmissionPort};
use rusqlite::ffi::quanta_capacity_vfs::close_owned_file;
use rustix::fs::{
    AtFlags, Dir, FileType, Mode, OFlags, RenameFlags, fstat, openat, renameat_with, statat,
    unlinkat,
};

use crate::publication_control_files::cold_control_open;
use crate::publication_file_capacity::{FileMutation, PublicationFileCapacity};
use crate::publication_file_deletions::PublicationFileDeletions;
use crate::publication_sidecar::sidecar_output_permit;
use quanta_index_searchd::app::runtime::StateRootDirectoryV1;

const BOOT_TEMP_PREFIX: &str = ".qiv-source-upload-";
const BOOT_TEMP_SUFFIX: &str = ".tmp";
const UPLOAD_DIRECTORY: &str = "source-publication-uploads";

fn contract(message: &str) -> CoreError {
    CoreError::InvalidContract(format!("source upload file admission: {message}"))
}

fn storage_io(context: &str, error: impl Into<io::Error>) -> CoreError {
    let error = error.into();
    CoreError::storage_io(&format!("source upload file admission {context}"), &error)
}

fn budget(message: &str) -> CoreError {
    CoreError::Typed {
        code: SearchPlaneErrorCodeV2::IngestResourceBudgetExceeded,
        message: format!("source upload file admission: {message}"),
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
    *message = format!("{message}; {context}: {additional}");
    primary
}

fn sync_directory(directory: &impl std::os::fd::AsFd) -> Result<(), CoreError> {
    rustix::fs::fsync(directory).map_err(|error| storage_io("fsync held parent", error))
}

fn exact_leaf(path: &Path) -> Result<&OsStr, CoreError> {
    path.file_name().ok_or_else(|| contract("path has no leaf"))
}

fn same_inode(
    directory: &impl std::os::fd::AsFd,
    leaf: &OsStr,
    file: &File,
) -> Result<bool, CoreError> {
    let named = statat(directory, Path::new(leaf), AtFlags::SYMLINK_NOFOLLOW)
        .map_err(|error| storage_io("stat held entry", error))?;
    let actual = fstat(file).map_err(|error| storage_io("stat original FD", error))?;
    Ok(named.st_dev == actual.st_dev
        && named.st_ino == actual.st_ino
        && named.st_nlink == 1
        && actual.st_nlink == 1
        && FileType::from_raw_mode(named.st_mode) == FileType::RegularFile)
}

fn require_named_upload_directory(
    root: &StateRootDirectoryV1,
    directory: &File,
) -> Result<(), CoreError> {
    root.require_current_v1()?;
    let pinned =
        fstat(directory).map_err(|error| storage_io("stat pinned upload directory", error))?;
    let named = statat(root, UPLOAD_DIRECTORY, AtFlags::SYMLINK_NOFOLLOW)
        .map_err(|error| storage_io("stat named upload directory", error))?;
    if FileType::from_raw_mode(pinned.st_mode) != FileType::Directory
        || FileType::from_raw_mode(named.st_mode) != FileType::Directory
        || (pinned.st_dev, pinned.st_ino) != (named.st_dev, named.st_ino)
    {
        return Err(contract("upload directory was replaced"));
    }
    Ok(())
}

fn body_path(root: &Path, path: &Path) -> Result<u64, CoreError> {
    if path.parent() != Some(root) || !path.is_absolute() {
        return Err(contract(
            "body is not a direct child of the leased upload root",
        ));
    }
    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| contract("body name is not UTF-8"))?;
    let Some((digest, width)) = name
        .strip_suffix(".cbor")
        .and_then(|name| name.split_once('-'))
    else {
        return Err(contract("body name is not canonical"));
    };
    if digest.len() != 64
        || width.len() != 16
        || !digest
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        || !width
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        || root.join(name).as_os_str() != path.as_os_str()
    {
        return Err(contract("body name is not canonical lowercase hex"));
    }
    u64::from_str_radix(width, 16).map_err(|error| {
        CoreError::InvalidContract(format!("source upload encoded body width: {error}"))
    })
}

fn private_regular(metadata: &Metadata, expected: u64) -> Result<(), CoreError> {
    if !metadata.is_file()
        || metadata.len() != expected
        || metadata.nlink() != 1
        || metadata.uid() != rustix::process::geteuid().as_raw()
        || metadata.mode() & 0o077 != 0
    {
        return Err(contract("body FD identity, width, or privacy changed"));
    }
    Ok(())
}

struct HeldBody {
    parent: Arc<PublicationAllowance>,
    declared: u64,
    active: bool,
}

struct BodyRegistryState {
    entries: Mutex<BTreeMap<PathBuf, HeldBody>>,
    poisoned: AtomicBool,
}

type BodyRegistry = Arc<BodyRegistryState>;

/// One pre-event upload policy backed by the same binder used by publication.
pub struct CanonicalSourceUploadFileAdmission {
    binder: Arc<PublicationFileCapacity>,
    deletions: Arc<PublicationFileDeletions>,
    held_root: Arc<StateRootDirectoryV1>,
    upload_directory: Mutex<Option<Arc<File>>>,
    root: PathBuf,
    max_body_bytes: u64,
    max_held_bodies: usize,
    bodies: BodyRegistry,
}

impl CanonicalSourceUploadFileAdmission {
    pub fn new(
        deletions: Arc<PublicationFileDeletions>,
        held_root: StateRootDirectoryV1,
        max_body_bytes: u64,
        max_held_bodies: usize,
        max_boot_temp_scan_entries: usize,
    ) -> Result<Self, CoreError> {
        if max_body_bytes == 0 || max_held_bodies == 0 || max_boot_temp_scan_entries == 0 {
            return Err(contract("all upload limits must be positive"));
        }
        let binder = deletions.binder();
        binder.validate_current_state_root()?;
        held_root.require_current_v1()?;
        let pinned = fstat(&held_root).map_err(|error| storage_io("stat held root", error))?;
        let current = statat(
            rustix::fs::CWD,
            binder.state_root(),
            AtFlags::SYMLINK_NOFOLLOW,
        )
        .map_err(|error| storage_io("stat binder root", error))?;
        if (pinned.st_dev, pinned.st_ino) != (current.st_dev, current.st_ino) {
            return Err(contract("held root differs from binder root"));
        }
        let result = Self {
            root: binder.state_root().join(UPLOAD_DIRECTORY),
            binder,
            deletions,
            held_root: Arc::new(held_root),
            upload_directory: Mutex::new(None),
            max_body_bytes,
            max_held_bodies,
            bodies: Arc::new(BodyRegistryState {
                entries: Mutex::new(BTreeMap::new()),
                poisoned: AtomicBool::new(false),
            }),
        };
        result.reconcile_boot_temporaries(max_boot_temp_scan_entries)?;
        Ok(result)
    }

    fn validate_root(&self, root: &Path) -> Result<(), CoreError> {
        if self.bodies.poisoned.load(Ordering::Acquire) {
            return Err(contract("upload body registry is poisoned"));
        }
        if root != self.root {
            return Err(contract("upload root differs from the held state root"));
        }
        self.binder.validate_current_state_root()?;
        self.held_root.require_current_v1()?;
        let pinned =
            fstat(self.held_root.as_ref()).map_err(|error| storage_io("stat held root", error))?;
        let current = statat(
            rustix::fs::CWD,
            self.binder.state_root(),
            AtFlags::SYMLINK_NOFOLLOW,
        )
        .map_err(|error| storage_io("stat binder root", error))?;
        if (pinned.st_dev, pinned.st_ino) != (current.st_dev, current.st_ino) {
            return Err(contract("held root identity changed"));
        }
        Ok(())
    }

    fn held_upload_directory(&self) -> Result<Arc<File>, CoreError> {
        self.validate_root(&self.root)?;
        let mut opened = Some(File::from(
            openat(
                self.held_root.as_ref(),
                UPLOAD_DIRECTORY,
                OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
                Mode::empty(),
            )
            .map_err(|error| storage_io("open upload directory from held root", error))?,
        ));
        let outcome = (|| {
            let actual = opened
                .as_ref()
                .ok_or_else(|| contract("opened upload directory disappeared"))?
                .metadata()
                .map_err(|error| storage_io("stat upload directory", error))?;
            let named = statat(
                self.held_root.as_ref(),
                UPLOAD_DIRECTORY,
                AtFlags::SYMLINK_NOFOLLOW,
            )
            .map_err(|error| storage_io("stat named upload directory", error))?;
            if !actual.is_dir()
                || FileType::from_raw_mode(named.st_mode) != FileType::Directory
                || actual.uid() != rustix::process::geteuid().as_raw()
                || actual.mode() & 0o077 != 0
                || (actual.dev(), actual.ino())
                    != (
                        crate::publication_inventory::checked_device_id(named.st_dev)
                            .map_err(|error| contract(&format!("directory device: {error}")))?,
                        named.st_ino,
                    )
            {
                return Err(contract("upload directory identity or privacy changed"));
            }
            let mut held = self.upload_directory.lock().map_err(|error| {
                CoreError::Storage(format!("source upload held directory lock: {error}"))
            })?;
            if let Some(prior) = held.as_ref() {
                let original = prior
                    .metadata()
                    .map_err(|error| storage_io("stat pinned upload directory", error))?;
                if (original.dev(), original.ino()) != (actual.dev(), actual.ino()) {
                    return Err(contract("upload directory was replaced"));
                }
                let answer = Arc::clone(prior);
                drop(held);
                return Ok(answer);
            }
            let answer = Arc::new(
                opened
                    .take()
                    .ok_or_else(|| contract("opened upload directory disappeared"))?,
            );
            *held = Some(Arc::clone(&answer));
            drop(held);
            Ok(answer)
        })();
        let closed = opened.map_or(Ok(()), |file| {
            close_owned_file(file)
                .map_err(|error| storage_io("close upload directory observer", error))
        });
        if closed.is_err() {
            self.binder.mark_close_unwitnessed();
        }
        match (outcome, closed) {
            (Ok(answer), Ok(())) => {
                self.validate_root(&self.root)?;
                Ok(answer)
            }
            (Err(primary), Ok(())) => Err(primary),
            (Ok(_), Err(close)) => Err(close),
            (Err(primary), Err(close)) => {
                Err(append_custody(primary, "directory retirement", &close))
            }
        }
    }

    fn validate_body(&self, path: &Path, declared: u64) -> Result<(), CoreError> {
        self.validate_root(&self.root)?;
        if declared == 0 || declared > self.max_body_bytes {
            return Err(budget("declared body exceeds configured maximum"));
        }
        if body_path(&self.root, path)? != declared {
            return Err(contract("body bound differs from its canonical pathname"));
        }
        Ok(())
    }

    fn reconcile_boot_temporaries(&self, max_entries: usize) -> Result<(), CoreError> {
        self.deletions.reconcile_retired_control_creates()?;
        if !self.binder.is_quiescent()? {
            return Err(contract("boot temp scan requires no live writer or reader"));
        }
        let mut seen = 0_usize;
        let mut names = Vec::new();
        let mut entries = Dir::read_from(self.held_root.as_ref())
            .map_err(|error| storage_io("scan held state root", error))?;
        while let Some(entry) = entries.read() {
            let entry = entry.map_err(|error| storage_io("read boot temp entry", error))?;
            let raw = entry.file_name().to_bytes();
            if raw == b"." || raw == b".." {
                continue;
            }
            seen = seen
                .checked_add(1)
                .ok_or_else(|| budget("boot scan entry overflow"))?;
            if seen > max_entries {
                return Err(budget("boot temp scan exceeds declared entry bound"));
            }
            let name = OsString::from_vec(raw.to_vec());
            let Some(name) = name.to_str() else {
                if raw.starts_with(BOOT_TEMP_PREFIX.as_bytes()) {
                    return Err(contract("non-UTF-8 upload temporary name"));
                }
                continue;
            };
            if !name.starts_with(BOOT_TEMP_PREFIX) {
                continue;
            }
            let Some(nonce) = name
                .strip_prefix(BOOT_TEMP_PREFIX)
                .and_then(|tail| tail.strip_suffix(BOOT_TEMP_SUFFIX))
            else {
                return Err(contract("foreign upload temporary name"));
            };
            if nonce.len() != 32
                || !nonce
                    .bytes()
                    .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
            {
                return Err(contract("malformed upload temporary name"));
            }
            names.push(name.to_owned());
        }
        // Complete the bounded namespace scan and authenticate every original
        // inode before a failed constructor can remove any staging file.
        let mut orphaned = Vec::with_capacity(names.len());
        for name in names {
            let path = self.binder.state_root().join(&name);
            let witness = File::from(
                openat(
                    self.held_root.as_ref(),
                    Path::new(&name),
                    OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::NONBLOCK | OFlags::CLOEXEC,
                    Mode::empty(),
                )
                .map_err(|error| storage_io("open boot upload temporary", error))?,
            );
            let observed = (|| {
                let metadata = witness
                    .metadata()
                    .map_err(|error| storage_io("stat boot upload temporary", error))?;
                private_regular(&metadata, metadata.len())?;
                let expected = self
                    .binder
                    .path_observation(&path)?
                    .ok_or_else(|| contract("boot temporary is absent from sealed inventory"))?;
                if expected.inode.device != metadata.dev()
                    || expected.inode.inode != metadata.ino()
                    || expected.bytes != metadata.len()
                {
                    return Err(contract("boot temporary differs from seeded inode"));
                }
                Ok(expected)
            })();
            let closed = close_owned_file(witness)
                .map_err(|error| storage_io("close boot upload temporary observer", error));
            if closed.is_err() {
                self.binder.mark_close_unwitnessed();
            }
            let expected = match (observed, closed) {
                (Ok(expected), Ok(())) => expected,
                (Err(primary), Ok(())) => return Err(primary),
                (Ok(_), Err(close)) => return Err(close),
                (Err(primary), Err(close)) => {
                    return Err(append_custody(primary, "observer retirement", &close));
                }
            };
            orphaned.push((path, expected.inode, expected.bytes));
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

    fn reserve_body(
        &self,
        path: &Path,
        declared: u64,
        existing: Option<u64>,
    ) -> Result<Arc<PublicationAllowance>, CoreError> {
        let mut bodies = self.bodies.entries.lock().map_err(|error| {
            self.bodies.poisoned.store(true, Ordering::Release);
            CoreError::Storage(format!("source upload body registry: {error}"))
        })?;
        if self.bodies.poisoned.load(Ordering::Acquire) {
            return Err(contract("upload body registry is poisoned"));
        }
        if let Some(body) = bodies.get_mut(path) {
            if body.declared != declared || body.active {
                return Err(contract("body parent changed or another writer is active"));
            }
            self.binder.validate_held_allowance(&body.parent)?;
            body.active = true;
            let parent = Arc::clone(&body.parent);
            drop(bodies);
            return Ok(parent);
        }
        if bodies.len() >= self.max_held_bodies {
            return Err(budget("held upload body limit exceeded"));
        }
        let parent = match existing {
            None => self.binder.reserve_publication_allowance(declared, 1)?,
            Some(length) => self
                .binder
                .reserve_existing_growth_allowance(declared.saturating_sub(length))?,
        };
        let _prior = bodies.insert(
            path.to_owned(),
            HeldBody {
                parent: Arc::clone(&parent),
                declared,
                active: true,
            },
        );
        drop(bodies);
        Ok(parent)
    }

    fn release_body(&self, path: &Path, complete: bool) -> Result<(), CoreError> {
        if self.bodies.poisoned.load(Ordering::Acquire) {
            return Err(contract("upload body registry is poisoned"));
        }
        let mut bodies = self.bodies.entries.lock().map_err(|error| {
            self.bodies.poisoned.store(true, Ordering::Release);
            CoreError::Storage(format!("source upload body registry release: {error}"))
        })?;
        if complete {
            let _removed = bodies.remove(path);
        } else {
            let body = bodies.get_mut(path).ok_or_else(|| {
                self.bodies.poisoned.store(true, Ordering::Release);
                contract("active body parent disappeared")
            })?;
            if !body.active {
                self.bodies.poisoned.store(true, Ordering::Release);
                return Err(contract("active body parent was already released"));
            }
            body.active = false;
        }
        drop(bodies);
        Ok(())
    }

    fn private_temp(&self) -> Result<PathBuf, CoreError> {
        let mut nonce = [0_u8; 16];
        let mut entropy = File::open("/dev/urandom")
            .map_err(|error| storage_io("open upload nonce source", error))?;
        let drawn = entropy
            .read_exact(&mut nonce)
            .map_err(|error| storage_io("generate upload temp nonce", error));
        let closed = close_owned_file(entropy)
            .map_err(|error| storage_io("close upload nonce source", error));
        match (drawn, closed) {
            (Ok(()), Ok(())) => {}
            (Err(primary), Ok(())) | (Ok(()), Err(primary)) => return Err(primary),
            (Err(primary), Err(close)) => {
                return Err(append_custody(primary, "nonce source close", &close));
            }
        }
        let mut hex = String::with_capacity(32);
        for byte in nonce {
            use std::fmt::Write as _;
            write!(&mut hex, "{byte:02x}").map_err(|error| {
                CoreError::Storage(format!("format upload temp nonce: {error}"))
            })?;
        }
        Ok(self
            .binder
            .state_root()
            .join(format!("{BOOT_TEMP_PREFIX}{hex}{BOOT_TEMP_SUFFIX}")))
    }

    fn reconcile_retired_resize_for_reader(&self, path: &Path) -> Result<(), CoreError> {
        if !self.binder.has_retired_safe_resize_pending(path)? {
            return Ok(());
        }
        // The binder checks this original nofollow FD against its pending
        // inode and anchored pathname, then consumes and checked-closes it.
        let directory = self.held_upload_directory()?;
        let witness = File::from(
            openat(
                directory.as_ref(),
                Path::new(exact_leaf(path)?),
                OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::NONBLOCK | OFlags::CLOEXEC,
                Mode::empty(),
            )
            .map_err(|error| storage_io("open retired resize witness", error))?,
        );
        if let Err(primary) = self.held_upload_directory() {
            return Err(self.close_rejected(witness, primary));
        }
        let _retained = self.binder.recover_pending_with_fd(path, path, witness)?;
        Ok(())
    }

    fn close_rejected(&self, writer: File, primary: CoreError) -> CoreError {
        match close_owned_file(writer) {
            Ok(()) => primary,
            Err(error) => {
                self.binder.mark_close_unwitnessed();
                append_custody(
                    primary,
                    "rejected original FD close",
                    &storage_io("close writer", error),
                )
            }
        }
    }
}

impl SourcePublicationUploadFileAdmissionPort for CanonicalSourceUploadFileAdmission {
    fn validate_root_and_reconcile(&self, upload_root: &Path) -> Result<(), CoreError> {
        self.validate_root(upload_root)?;
        self.deletions.reconcile_retired_control_creates()?;
        self.deletions.retry_pending_deletion()
    }

    fn begin_write(
        &self,
        path: &Path,
        declared_body_bytes: u64,
        expected_length: Option<u64>,
        target_end: u64,
    ) -> Result<Box<dyn PublicationControlFileMutation>, CoreError> {
        self.validate_body(path, declared_body_bytes)?;
        if target_end == 0 || target_end > declared_body_bytes {
            return Err(budget("write target exceeds declared complete body"));
        }
        if expected_length.is_some_and(|length| length >= target_end) {
            return Err(contract("upload writer requires positive growth"));
        }
        self.validate_root_and_reconcile(&self.root)?;
        if expected_length.is_some() {
            self.reconcile_retired_resize_for_reader(path)?;
        }
        let observed = self.binder.path_observation(path)?;
        if observed.map(|value| value.bytes) != expected_length {
            return Err(contract("body birth/existing width differs from request"));
        }
        let parent = self.reserve_body(path, declared_body_bytes, expected_length)?;
        let result = if let Some(expected) = expected_length {
            self.begin_append(path, declared_body_bytes, expected, target_end, parent)
        } else {
            self.begin_create(path, declared_body_bytes, target_end, parent)
        };
        match result {
            Ok(writer) => {
                let owned: Box<dyn PublicationControlFileMutation> = Box::new(writer);
                Ok(owned)
            }
            Err(primary) => {
                // A refused birth has no reusable body parent. A started
                // child retains its pending charge after this parent closes.
                Err(match self.release_body(path, expected_length.is_none()) {
                    Ok(()) => primary,
                    Err(release) => append_custody(primary, "body parent release", &release),
                })
            }
        }
    }

    fn begin_reader_open(
        &self,
        path: &Path,
    ) -> Result<Box<dyn PublicationControlOpenTicket>, CoreError> {
        let declared = body_path(&self.root, path)?;
        self.validate_body(path, declared)?;
        self.validate_root_and_reconcile(&self.root)?;
        self.reconcile_retired_resize_for_reader(path)?;
        let _confirmed_directory = self.held_upload_directory()?;
        Ok(cold_control_open(self.binder.begin_reader_open(path)?))
    }

    fn delete_file_and_sync(
        &self,
        path: &Path,
        expected_inode: InodeKey,
        expected_length: u64,
    ) -> Result<(), CoreError> {
        let declared = body_path(&self.root, path)?;
        self.validate_body(path, declared)?;
        let _confirmed_directory = self.held_upload_directory()?;
        {
            let bodies = self.bodies.entries.lock().map_err(|error| {
                self.bodies.poisoned.store(true, Ordering::Release);
                CoreError::Storage(format!("source upload delete registry: {error}"))
            })?;
            if bodies.get(path).is_some_and(|body| body.active) {
                return Err(contract("active upload writer cannot be deleted"));
            }
        }
        self.deletions
            .delete_owned_file_and_sync(path, expected_inode, expected_length)?;
        self.release_body(path, true)?;
        Ok(())
    }
}

impl CanonicalSourceUploadFileAdmission {
    fn begin_create(
        &self,
        path: &Path,
        declared: u64,
        target_end: u64,
        parent: Arc<PublicationAllowance>,
    ) -> Result<UploadMutation, CoreError> {
        let directory = self.held_upload_directory()?;
        let temporary = self.private_temp()?;
        let temp_name = exact_leaf(&temporary)?;
        let request =
            PublicationOutputReservation::new(PublicationOutputClass::Control, target_end, 1)?;
        let mut permit = sidecar_output_permit(Arc::clone(&self.deletions), parent, request)?;
        permit.bind_output(&temporary, path)?;
        permit.creation_started()?;
        let writer = match openat(
            self.held_root.as_ref(),
            Path::new(temp_name),
            OFlags::RDWR | OFlags::CREATE | OFlags::EXCL | OFlags::NOFOLLOW | OFlags::CLOEXEC,
            Mode::from_raw_mode(0o600),
        ) {
            Ok(fd) => File::from(fd),
            Err(error) => {
                let primary = storage_io("create private staging body", error);
                return Err(
                    match statat(
                        self.held_root.as_ref(),
                        Path::new(temp_name),
                        AtFlags::SYMLINK_NOFOLLOW,
                    ) {
                        Err(rustix::io::Errno::NOENT) => {
                            match sync_directory(self.held_root.as_ref())
                                .and_then(|()| permit.confirmed_uncreated_absent_and_synced())
                            {
                                Ok(()) => primary,
                                Err(cleanup) => {
                                    append_custody(primary, "uncreated staging proof", &cleanup)
                                }
                            }
                        }
                        Ok(_) => append_custody(
                            primary,
                            "occupied staging name",
                            &contract("temp name is occupied"),
                        ),
                        Err(inspect) => append_custody(
                            primary,
                            "inspect staging name",
                            &storage_io("stat failed create", inspect),
                        ),
                    },
                );
            }
        };
        if let Err(primary) = permit.created(&writer, &temporary) {
            return Err(match permit.cancel_rejected_created(writer) {
                Ok(()) => primary,
                Err(cleanup) => append_custody(primary, "rejected birth cleanup", &cleanup),
            });
        }
        Ok(UploadMutation {
            bodies: Arc::clone(&self.bodies),
            path: path.to_owned(),
            writer: Some(writer),
            kind: UploadMutationKind::Create { permit, temporary },
            held_root: Arc::clone(&self.held_root),
            upload_directory: directory,
            declared,
            base_length: 0,
            target_end,
            published: false,
            complete: false,
            released: false,
        })
    }

    fn begin_append(
        &self,
        path: &Path,
        declared: u64,
        expected: u64,
        target_end: u64,
        parent: Arc<PublicationAllowance>,
    ) -> Result<UploadMutation, CoreError> {
        let directory = self.held_upload_directory()?;
        let writer = File::from(
            openat(
                directory.as_ref(),
                Path::new(exact_leaf(path)?),
                OFlags::RDWR | OFlags::NOFOLLOW | OFlags::NONBLOCK | OFlags::CLOEXEC,
                Mode::empty(),
            )
            .map_err(|error| storage_io("open original upload writer", error))?,
        );
        let setup = (|| {
            let _confirmed_directory = self.held_upload_directory()?;
            private_regular(
                &writer
                    .metadata()
                    .map_err(|error| storage_io("stat upload writer", error))?,
                expected,
            )?;
            self.binder
                .begin_resize_with_allowance(path, &writer, target_end, parent)
        })();
        let mut mutation = match setup {
            Ok(mutation) => mutation,
            Err(primary) => return Err(self.close_rejected(writer, primary)),
        };
        if let Err(primary) = mutation.mutation_started() {
            return Err(match mutation.confirmed_safe_writer_fd_retired(writer) {
                Ok(()) => primary,
                Err(close) => append_custody(primary, "resize writer retirement", &close),
            });
        }
        let mut writer = writer;
        if let Err(primary) = writer
            .seek(SeekFrom::Start(expected))
            .map_err(|error| storage_io("seek original writer", error))
        {
            return Err(match mutation.confirmed_safe_writer_fd_retired(writer) {
                Ok(()) => primary,
                Err(close) => append_custody(primary, "resize writer retirement", &close),
            });
        }
        Ok(UploadMutation {
            bodies: Arc::clone(&self.bodies),
            path: path.to_owned(),
            writer: Some(writer),
            kind: UploadMutationKind::Append {
                mutation: Box::new(mutation),
            },
            held_root: Arc::clone(&self.held_root),
            upload_directory: directory,
            declared,
            base_length: expected,
            target_end,
            published: false,
            complete: false,
            released: false,
        })
    }
}

enum UploadMutationKind {
    Create {
        permit: Box<dyn PublicationOutputPermit>,
        temporary: PathBuf,
    },
    Append {
        // The body registry bounds active writers; the child mutation keeps
        // its original FD and allowance custody inside this allocation.
        mutation: Box<FileMutation>,
    },
}

struct UploadMutation {
    bodies: BodyRegistry,
    held_root: Arc<StateRootDirectoryV1>,
    upload_directory: Arc<File>,
    path: PathBuf,
    writer: Option<File>,
    kind: UploadMutationKind,
    declared: u64,
    base_length: u64,
    target_end: u64,
    published: bool,
    complete: bool,
    released: bool,
}

impl UploadMutation {
    fn release_book(&mut self) -> Result<(), CoreError> {
        if self.released {
            return Ok(());
        }
        if self.bodies.poisoned.load(Ordering::Acquire) {
            return Err(contract("upload body registry is poisoned"));
        }
        let mut bodies = self.bodies.entries.lock().map_err(|error| {
            self.bodies.poisoned.store(true, Ordering::Release);
            CoreError::Storage(format!("source upload writer book release: {error}"))
        })?;
        if self.bodies.poisoned.load(Ordering::Acquire) {
            return Err(contract("upload body registry is poisoned"));
        }
        let body = bodies.get_mut(&self.path).ok_or_else(|| {
            self.bodies.poisoned.store(true, Ordering::Release);
            contract("writer body parent disappeared")
        })?;
        if !body.active || body.declared != self.declared {
            self.bodies.poisoned.store(true, Ordering::Release);
            return Err(contract("writer body parent changed before release"));
        }
        if self.complete {
            let _removed = bodies.remove(&self.path);
        } else {
            body.active = false;
        }
        self.released = true;
        drop(bodies);
        Ok(())
    }

    fn writer_mut(&mut self) -> io::Result<&mut File> {
        self.writer
            .as_mut()
            .ok_or_else(|| io::Error::other(contract("writer FD retired")))
    }

    fn close_pending(&mut self) -> Result<(), CoreError> {
        let Some(writer) = self.writer.take() else {
            return Ok(());
        };
        match &mut self.kind {
            UploadMutationKind::Create { permit, .. } => {
                permit.retire_writer_preserving_pending(writer)
            }
            UploadMutationKind::Append { mutation } => {
                mutation.confirmed_safe_writer_fd_retired(writer)
            }
        }
    }

    fn cleanup_unpublished(&mut self) -> Result<(), CoreError> {
        let UploadMutationKind::Create { permit, temporary } = &mut self.kind else {
            return self.close_pending();
        };
        let writer = self
            .writer
            .take()
            .ok_or_else(|| contract("created writer FD absent"))?;
        let cleanup = (|| {
            self.held_root.require_current_v1()?;
            let leaf = exact_leaf(temporary)?;
            if !same_inode(self.held_root.as_ref(), leaf, &writer)? {
                return Err(contract("staging pathname differs from original writer"));
            }
            unlinkat(self.held_root.as_ref(), Path::new(leaf), AtFlags::empty())
                .map_err(|error| storage_io("unlink exact staging body", error))?;
            sync_directory(self.held_root.as_ref())
        })();
        match cleanup {
            Ok(()) => permit.confirmed_safe_absent_after_writer_close(writer),
            Err(primary) => match permit.retire_writer_preserving_pending(writer) {
                Ok(()) => Err(primary),
                Err(retirement) => Err(append_custody(primary, "writer retirement", &retirement)),
            },
        }
    }

    fn finish(&mut self) -> Result<(), CoreError> {
        let actual = self
            .writer
            .as_ref()
            .ok_or_else(|| contract("writer FD absent"))?;
        private_regular(
            &actual
                .metadata()
                .map_err(|error| storage_io("stat original writer", error))?,
            self.target_end,
        )?;
        match &mut self.kind {
            UploadMutationKind::Create { permit, temporary } => {
                actual
                    .sync_all()
                    .map_err(|error| storage_io("fsync staging writer", error))?;
                permit.before_replace()?;
                require_named_upload_directory(
                    self.held_root.as_ref(),
                    self.upload_directory.as_ref(),
                )?;
                renameat_with(
                    self.held_root.as_ref(),
                    Path::new(exact_leaf(temporary)?),
                    self.upload_directory.as_ref(),
                    Path::new(exact_leaf(&self.path)?),
                    RenameFlags::NOREPLACE,
                )
                .map_err(|error| storage_io("publish body without replacement", error))?;
                self.published = true;
                require_named_upload_directory(
                    self.held_root.as_ref(),
                    self.upload_directory.as_ref(),
                )?;
                sync_directory(self.upload_directory.as_ref())?;
                sync_directory(self.held_root.as_ref())?;
                let writer = self
                    .writer
                    .take()
                    .ok_or_else(|| contract("original writer FD absent"))?;
                permit.committed(writer, self.target_end)?;
            }
            UploadMutationKind::Append { mutation } => {
                let _retained = mutation.settle_after_file_and_parent_sync(actual)?;
                let writer = self
                    .writer
                    .take()
                    .ok_or_else(|| contract("original writer FD absent"))?;
                mutation.confirmed_safe_writer_fd_retired(writer)?;
                let lease = mutation.take_owned_lease()?;
                lease.retire()?;
            }
        }
        self.complete = self.target_end == self.declared;
        Ok(())
    }
}

impl Read for UploadMutation {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        self.writer_mut()?.read(buf)
    }
}

impl Seek for UploadMutation {
    fn seek(&mut self, pos: SeekFrom) -> io::Result<u64> {
        self.writer_mut()?.seek(pos)
    }
}

impl Write for UploadMutation {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        let position = self.writer_mut()?.stream_position()?;
        if position < self.base_length {
            return Err(io_error(contract(
                "write would change acknowledged body prefix",
            )));
        }
        let end = position
            .checked_add(u64::try_from(buf.len()).map_err(io::Error::other)?)
            .ok_or_else(|| io_error(budget("write offset overflow")))?;
        if end > self.target_end {
            return Err(io_error(budget("write exceeds admitted target")));
        }
        self.writer_mut()?.write(buf)
    }
    fn flush(&mut self) -> io::Result<()> {
        self.writer_mut()?.flush()
    }
}

impl PublicationControlFileMutation for UploadMutation {
    fn metadata(&self) -> Result<Metadata, CoreError> {
        self.writer
            .as_ref()
            .ok_or_else(|| contract("original writer FD retired"))?
            .metadata()
            .map_err(|error| storage_io("writer metadata", error))
    }

    fn set_len(&mut self, _length: u64) -> Result<(), CoreError> {
        Err(contract(
            "source upload body cannot be truncated or preextended",
        ))
    }

    fn sync_and_settle(mut self: Box<Self>) -> Result<(), CoreError> {
        if let Err(primary) = self.finish() {
            let retirement = if self.published {
                self.close_pending()
            } else {
                self.cleanup_unpublished()
            };
            if matches!(&self.kind, UploadMutationKind::Create { .. })
                && !self.published
                && retirement.is_ok()
            {
                self.complete = true;
            }
            let primary = match retirement {
                Ok(()) => primary,
                Err(cleanup) => append_custody(primary, "failed write cleanup", &cleanup),
            };
            return Err(match self.release_book() {
                Ok(()) => primary,
                Err(release) => append_custody(primary, "body book release", &release),
            });
        }
        self.release_book()
    }

    fn retire_preserving_pending(mut self: Box<Self>) -> Result<(), CoreError> {
        let retirement = if self.published {
            self.close_pending()
        } else {
            let cleanup = self.cleanup_unpublished();
            if matches!(&self.kind, UploadMutationKind::Create { .. }) && cleanup.is_ok() {
                self.complete = true;
            }
            cleanup
        };
        match (retirement, self.release_book()) {
            (Ok(()), Ok(())) => Ok(()),
            (Err(primary), Ok(())) => Err(primary),
            (Ok(()), Err(release)) => Err(release),
            (Err(primary), Err(release)) => {
                Err(append_custody(primary, "body book release", &release))
            }
        }
    }
}

impl Drop for UploadMutation {
    fn drop(&mut self) {
        if self.writer.is_some() {
            let _retirement = if self.published {
                self.close_pending()
            } else {
                let cleanup = self.cleanup_unpublished();
                if matches!(&self.kind, UploadMutationKind::Create { .. }) && cleanup.is_ok() {
                    self.complete = true;
                }
                cleanup
            };
        }
        if !self.released && self.release_book().is_err() {
            self.bodies.poisoned.store(true, Ordering::Release);
        }
    }
}

#[cfg(test)]
mod tests;
