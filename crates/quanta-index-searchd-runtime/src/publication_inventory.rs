//! Fail-closed boot inventory and shared native device-number normalization.
//!
//! This records logical file lengths, not allocated blocks or free-space reserve.
//! The caller must hold the state-root lease and run this before opening writers.
//! The opened root is authenticated against the directory and lock-file FDs
//! retained by that lease; a canonical pathname alone is not its identity.
//! The resulting capacity is a logical file-length authority. Installing it
//! still requires the shared estimator and all controlled writer bindings.

use std::collections::BTreeMap;
use std::ffi::{OsStr, OsString};
use std::fs::File;
use std::os::fd::AsFd as _;
use std::os::unix::ffi::OsStringExt as _;
use std::os::unix::fs::MetadataExt as _;
use std::path::{Path, PathBuf};

use quanta_index_contract::SearchPlaneErrorCodeV2;
use quanta_index_core::publication_capacity::{
    InodeKey, PublicationCapacity, PublicationCapacityLedger,
};
use quanta_index_core::{CoreError, RequestBudgetV1};
use quanta_index_searchd::app::runtime::StateRootLease;
use rustix::fs::{AtFlags, Dir, FileType, Mode, OFlags, openat, statat};

#[path = "publication_inventory/admitted.rs"]
mod admitted;
pub(crate) use admitted::{AdmittedBootInventoryV1, InventoryMemory};
use admitted::{InventoryControl, ScanFile};

/// Normalize native stat device numbers without losing signed-platform errors.
pub(crate) fn checked_device_id<T: TryInto<u64>>(device: T) -> Result<u64, T::Error> {
    device.try_into()
}

/// Linux nlink_t is u64; Darwin uses a narrower unsigned field.
pub(crate) fn link_count_u64<T: Into<u64>>(links: T) -> u64 {
    links.into()
}

/// All ceilings are supplied by the service configuration; no implicit scan
/// or publication capacity is inferred from observed usage.
#[derive(Clone, Copy, serde::Deserialize)]
#[serde(deny_unknown_fields)]
#[expect(
    clippy::struct_field_names,
    reason = "public limit fields deliberately name each configured maximum"
)]
pub struct PublicationBootInventoryLimits {
    pub max_bytes: u64,
    pub max_files: u64,
    pub max_pending: u64,
    pub max_entries: u64,
    pub max_depth: u32,
    pub max_pending_frames: u32,
}

struct Frame<G> {
    directory: ScanFile,
    path: PathBuf,
    depth: usize,
    _memory: G,
}

struct Observed {
    bytes: u64,
    links: u64,
    aliases: u64,
}

fn inventory_error(detail: impl std::fmt::Display) -> CoreError {
    CoreError::Storage(format!("publication boot inventory: {detail}"))
}

fn inventory_capacity(detail: &'static str) -> CoreError {
    CoreError::Typed {
        code: SearchPlaneErrorCodeV2::IngestResourceBudgetExceeded,
        message: format!("publication boot inventory: {detail}"),
    }
}

fn open_child(parent: &File, name: &OsStr, directory: bool) -> Result<File, CoreError> {
    let mut flags = OFlags::RDONLY | OFlags::CLOEXEC | OFlags::NOFOLLOW | OFlags::NONBLOCK;
    if directory {
        flags |= OFlags::DIRECTORY;
    }
    openat(parent, Path::new(name), flags, Mode::empty())
        .map(File::from)
        .map_err(|error| {
            inventory_error(format!(
                "open entry {}: {error}",
                name.as_encoded_bytes().escape_ascii()
            ))
        })
}

fn inode(metadata: &std::fs::Metadata) -> InodeKey {
    InodeKey {
        device: metadata.dev(),
        inode: metadata.ino(),
    }
}

fn compare_entry(parent: &File, name: &OsStr, opened: &File) -> Result<(), CoreError> {
    let before = statat(parent, Path::new(name), AtFlags::SYMLINK_NOFOLLOW).map_err(|error| {
        inventory_error(format!(
            "stat entry {}: {error}",
            name.as_encoded_bytes().escape_ascii()
        ))
    })?;
    let metadata = opened.metadata().map_err(|error| {
        inventory_error(format!(
            "stat opened entry {}: {error}",
            name.as_encoded_bytes().escape_ascii()
        ))
    })?;
    let device = crate::publication_inventory::checked_device_id(before.st_dev)
        .map_err(|error| inventory_error(format!("entry device width: {error}")))?;
    let length = u64::try_from(before.st_size)
        .map_err(|error| inventory_error(format!("entry size width: {error}")))?;
    let observed = InodeKey {
        device,
        inode: before.st_ino,
    };
    if observed != inode(&metadata)
        || length != metadata.len()
        || link_count_u64(before.st_nlink) != metadata.nlink()
    {
        return Err(inventory_error(format!(
            "entry changed during scan: {}",
            name.as_encoded_bytes().escape_ascii()
        )));
    }
    Ok(())
}

/// Scan under the held lease and return a sealed ledger only after all paths,
/// alias counts, and ceilings have passed. A failed scan exposes no authority.
pub fn seed_publication_capacity(
    lease: &StateRootLease,
    state_root: &Path,
    limits: PublicationBootInventoryLimits,
    allowed_socket_entries: &[PathBuf],
    budget: &RequestBudgetV1,
) -> Result<PublicationCapacity, CoreError> {
    Ok(seed_with_control(
        lease,
        state_root,
        limits,
        allowed_socket_entries,
        &mut admitted::Ordinary::new(budget),
    )?
    .0)
}

/// Same canonical inode census, now funded by the ORIGINAL Control budget.
/// This does not create roots, default limits, a new execution or a Source permit.
pub(crate) fn seed_publication_capacity_admitted_v1(
    registration: &crate::publication_file_capacity::RecoveryReadRootRegistrationV1,
    limits: PublicationBootInventoryLimits,
    allowed_socket_entries: &[PathBuf],
    max_open_handles: usize,
    budget: &mut quanta_index_core::index::WorkBudget,
) -> Result<
    AdmittedBootInventoryV1,
    quanta_index_core::index::existing_file::ExistingFileReadFailureV1,
> {
    let lease = registration.lease()?;
    let state_root = lease.state_root_identity_v1();
    let mut control = admitted::Admitted::new(budget, max_open_handles, registration)?;
    let (capacity, observed_files, observed_entries, max_path_bytes) = seed_with_control(
        lease,
        state_root,
        limits,
        allowed_socket_entries,
        &mut control,
    )?;
    Ok(AdmittedBootInventoryV1 {
        capacity,
        observed_files,
        observed_entries,
        max_path_bytes,
        memory: control.memory,
    })
}
fn seed_with_control<C: InventoryControl>(
    lease: &StateRootLease,
    state_root: &Path,
    limits: PublicationBootInventoryLimits,
    allowed_socket_entries: &[PathBuf],
    control: &mut C,
) -> Result<(PublicationCapacity, u64, u64, usize), C::Error> {
    if limits.max_bytes == 0
        || limits.max_files == 0
        || limits.max_pending == 0
        || limits.max_entries == 0
        || limits.max_pending_frames == 0
    {
        return Err((CoreError::InvalidContract(
            "publication boot inventory limits must be positive".into(),
        ))
        .into());
    }
    control.retain(PublicationCapacityLedger::boot_state_allocation_bytes_v1()?)?;
    control.charge(
        u64::try_from(state_root.components().count())
            .map_err(|_| inventory_error("root component width"))?
            .checked_mul(8)
            .and_then(|n| n.checked_add(8))
            .ok_or_else(|| inventory_error("root admission width"))?,
        0,
    )?;
    lease.require_state_root_v1(state_root)?;
    control.checkpoint("publication boot inventory root")?;
    // Borrow the original leased FD, then admit an independently positioned scan FD.
    let root_custody = lease.borrow_root_directory_read_v1()?;
    let root = ScanFile::open(control, || {
        openat(
            root_custody.as_fd(),
            Path::new("."),
            OFlags::RDONLY | OFlags::CLOEXEC | OFlags::NOFOLLOW | OFlags::DIRECTORY,
            Mode::empty(),
        )
        .map(File::from)
        .map_err(|error| {
            inventory_error(format!(
                "open independently positioned leased root: {error}"
            ))
        })
    })?;
    let root_path_memory = control.temporary(
        u64::try_from(state_root.as_os_str().len())
            .map_err(|_| inventory_error("root path width"))?,
    )?;
    let mut stack = Vec::<Frame<C::Memory>>::new();
    admitted::vector_slot(&mut stack, control)?;
    stack.push(Frame {
        directory: root,
        path: state_root.to_path_buf(),
        depth: 0,
        _memory: root_path_memory,
    });
    let mut max_path_bytes = state_root.as_os_str().len();
    let mut entries_seen = 0_u64;
    let mut regular = BTreeMap::<InodeKey, Observed>::new();
    let mut used_bytes = 0_u64;
    let mut used_files = 0_u64;
    while let Some(frame) = stack.pop() {
        control.checkpoint("publication boot inventory directory")?;
        admitted::entries(&frame.directory, control, &mut |raw, control| {
            if raw == b"." || raw == b".." {
                return Ok(());
            }
            entries_seen = entries_seen
                .checked_add(1)
                .ok_or_else(|| inventory_error("entry count overflow"))?;
            if entries_seen > limits.max_entries {
                return Err((inventory_error("entry count exceeds configured bound")).into());
            }
            let path_bytes = frame
                .path
                .as_os_str()
                .len()
                .checked_add(raw.len())
                .and_then(|n| n.checked_add(1))
                .ok_or_else(|| inventory_error("inventory path width overflow"))?;
            let entry_memory = control.temporary(
                u64::try_from(
                    path_bytes
                        .checked_mul(2)
                        .and_then(|n| n.checked_add(raw.len()))
                        .ok_or_else(|| inventory_error("inventory entry allocation overflow"))?,
                )
                .map_err(|_| inventory_error("inventory entry allocation width"))?,
            )?;
            let name = OsString::from_vec(raw.to_vec());
            let path = frame.path.join(&name);
            max_path_bytes = max_path_bytes.max(path.as_os_str().len());
            control.charge(
                1,
                u64::try_from(std::mem::size_of::<rustix::fs::Stat>())
                    .map_err(|_| inventory_error("inventory stat width"))?,
            )?;
            let stat = statat(
                frame.directory.file()?,
                Path::new(&name),
                AtFlags::SYMLINK_NOFOLLOW,
            )
            .map_err(|error| inventory_error(format!("stat {}: {error}", path.display())))?;
            match FileType::from_raw_mode(stat.st_mode) {
                FileType::Directory => {
                    let depth = frame
                        .depth
                        .checked_add(1)
                        .ok_or_else(|| inventory_error("directory depth overflow"))?;
                    if depth
                        > usize::try_from(limits.max_depth)
                            .map_err(|error| inventory_error(format!("depth bound: {error}")))?
                        || stack.len()
                            >= usize::try_from(limits.max_pending_frames)
                                .map_err(|error| inventory_error(format!("frame bound: {error}")))?
                    {
                        return Err((inventory_error(
                            "directory depth or pending frames exceeds bound",
                        ))
                        .into());
                    }
                    let directory = ScanFile::open(control, || {
                        open_child(frame.directory.file()?, &name, true)
                    })?;
                    control.charge(2, 0)?;
                    compare_entry(frame.directory.file()?, &name, directory.file()?)?;
                    admitted::vector_slot(&mut stack, control)?;
                    stack.push(Frame {
                        directory,
                        path,
                        depth,
                        _memory: entry_memory,
                    });
                }
                FileType::RegularFile => {
                    let file = ScanFile::open(control, || {
                        open_child(frame.directory.file()?, &name, false)
                    })?;
                    control.charge(3, 0)?;
                    compare_entry(frame.directory.file()?, &name, file.file()?)?;
                    let metadata = file.file()?.metadata().map_err(|error| {
                        inventory_error(format!("stat file {}: {error}", path.display()))
                    })?;
                    let key = inode(&metadata);
                    if let Some(aliases) = regular.get_mut(&key) {
                        if aliases.bytes != metadata.len() || aliases.links != metadata.nlink() {
                            return Err((inventory_error(format!(
                                "conflicting hardlink width or count: {}",
                                path.display()
                            )))
                            .into());
                        }
                        aliases.aliases = aliases
                            .aliases
                            .checked_add(1)
                            .ok_or_else(|| inventory_error("hardlink alias count overflow"))?;
                    } else {
                        let next_bytes = used_bytes
                            .checked_add(metadata.len())
                            .ok_or_else(|| inventory_capacity("boot byte capacity overflow"))?;
                        if next_bytes > limits.max_bytes {
                            return Err((inventory_capacity("boot byte capacity exceeded")).into());
                        }
                        let next_files = used_files
                            .checked_add(1)
                            .ok_or_else(|| inventory_capacity("boot file capacity overflow"))?;
                        if next_files > limits.max_files {
                            return Err((inventory_capacity("boot file capacity exceeded")).into());
                        }
                        control.scratch(quanta_index_core::publication_capacity::publication_inventory_map_node_bytes_v1::<InodeKey,Observed>()?)?;
                        let _previous = regular.insert(
                            key,
                            Observed {
                                bytes: metadata.len(),
                                links: metadata.nlink(),
                                aliases: 1,
                            },
                        );
                        used_bytes = next_bytes;
                        used_files = next_files;
                    }
                }
                FileType::Socket
                    if allowed_socket_entries
                        .iter()
                        .any(|allowed| allowed.as_path() == path) =>
                {
                    // Socket inodes have no regular-file byte charge. The caller
                    // declares the exact service socket paths, including stale
                    // entries that its socket binder will authenticate/remove.
                }
                FileType::Symlink
                | FileType::Fifo
                | FileType::Socket
                | FileType::CharacterDevice
                | FileType::BlockDevice
                | FileType::Unknown => {
                    return Err((inventory_error(format!(
                        "symlink or unsupported entry type: {}",
                        path.display()
                    )))
                    .into());
                }
            }
            Ok(())
        })?;
    }
    admitted::require_retired(control)?;
    for (key, observed) in &regular {
        if observed.links != observed.aliases {
            return Err((inventory_error(format!(
                "external or changing hardlink count at inode {}:{}: nlink={} root_aliases={}",
                key.device, key.inode, observed.links, observed.aliases
            )))
            .into());
        }
    }
    control.charge(
        u64::try_from(state_root.components().count())
            .map_err(|_| inventory_error("root retirement component width"))?
            .checked_mul(16)
            .and_then(|n| n.checked_add(32))
            .ok_or_else(|| inventory_error("root retirement admission width"))?,
        0,
    )?;
    lease.require_state_root_v1(state_root)?;
    root_custody.require_current_v1()?;
    control.checkpoint("publication boot inventory seal")?;
    let mut ledger =
        PublicationCapacityLedger::new(limits.max_bytes, limits.max_files, limits.max_pending)?;
    for (key, observed) in regular {
        control.retain(PublicationCapacityLedger::boot_inode_allocation_bytes_v1()?)?;
        let _retained = ledger.seed_inode(key, observed.bytes)?;
    }
    let capacity = ledger.seal_boot()?;
    Ok((capacity, used_files, entries_seen, max_path_bytes))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fmt::Debug;
    use std::fs;
    use std::os::unix::fs::{PermissionsExt as _, symlink};
    use std::time::Duration;

    type TestResult = Result<(), Box<dyn std::error::Error>>;

    fn require(condition: bool, message: impl Into<String>) -> TestResult {
        if condition {
            Ok(())
        } else {
            Err(message.into().into())
        }
    }

    fn require_equal<T: Debug + PartialEq>(actual: &T, expected: &T, label: &str) -> TestResult {
        require(
            actual == expected,
            format!("{label}: expected {expected:?}, got {actual:?}"),
        )
    }

    fn root() -> Result<(tempfile::TempDir, PathBuf, StateRootLease), Box<dyn std::error::Error>> {
        let temporary = tempfile::tempdir()?;
        let state_root = temporary.path().join("state");
        fs::create_dir(&state_root)?;
        fs::set_permissions(&state_root, fs::Permissions::from_mode(0o700))?;
        let lease = StateRootLease::acquire(&state_root)?;
        Ok((temporary, state_root, lease))
    }

    fn limits() -> PublicationBootInventoryLimits {
        PublicationBootInventoryLimits {
            max_bytes: 1024,
            max_files: 16,
            max_pending: 16,
            max_entries: 32,
            max_depth: 4,
            max_pending_frames: 8,
        }
    }

    fn seed(
        lease: &StateRootLease,
        state_root: &Path,
        limits: PublicationBootInventoryLimits,
        budget: &RequestBudgetV1,
    ) -> Result<PublicationCapacity, CoreError> {
        seed_publication_capacity(lease, state_root, limits, &[], budget)
    }

    #[test]
    fn hardlinks_charge_once_and_zero_length_file_charges_one_slot() -> TestResult {
        let (_temporary, state_root, lease) = root()?;
        fs::write(state_root.join("a"), b"four")?;
        fs::hard_link(state_root.join("a"), state_root.join("b"))?;
        fs::write(state_root.join("empty"), b"")?;
        let authority = seed(&lease, &state_root, limits(), &RequestBudgetV1::unbounded())?;
        let usage = authority.snapshot()?;
        require_equal(&usage.used_bytes, &4, "hardlink inventory bytes")?;
        require_equal(&usage.used_files, &3, "hardlink inventory files")?; // lease file, hardlink inode, empty inode
        require_equal(&usage.retained_inodes, &3, "hardlink inventory inodes")?;
        Ok(())
    }

    #[test]
    fn authenticated_root_alias_seeds_the_pinned_directory() -> TestResult {
        let (temporary, state_root, lease) = root()?;
        fs::write(state_root.join("payload"), b"four")?;
        let alias = temporary.path().join("state-alias");
        symlink(&state_root, &alias)?;
        let canonical = seed(&lease, &state_root, limits(), &RequestBudgetV1::unbounded())?;
        let aliased = seed(&lease, &alias, limits(), &RequestBudgetV1::unbounded())?;
        let canonical_usage = canonical.snapshot()?;
        let alias_usage = aliased.snapshot()?;
        require_equal(&alias_usage.used_bytes, &4, "alias inventory bytes")?;
        require_equal(&alias_usage.used_files, &2, "alias inventory file count")?;
        require_equal(
            &alias_usage,
            &canonical_usage,
            "alias inventory matches pinned root",
        )?;
        Ok(())
    }

    #[test]
    fn byte_and_file_capacity_refuse_complete_boot_inventory() -> TestResult {
        let (_temporary, state_root, lease) = root()?;
        fs::write(state_root.join("four"), b"four")?;
        fs::write(state_root.join("empty"), b"")?;
        let mut byte_limit = limits();
        byte_limit.max_bytes = 3;
        require(
            seed(
                &lease,
                &state_root,
                byte_limit,
                &RequestBudgetV1::unbounded(),
            )
            .is_err(),
            "byte-limited boot inventory was accepted",
        )?;
        let mut file_limit = limits();
        file_limit.max_files = 2;
        require(
            seed(
                &lease,
                &state_root,
                file_limit,
                &RequestBudgetV1::unbounded(),
            )
            .is_err(),
            "file-limited boot inventory was accepted",
        )?;
        require_equal(
            &fs::read(state_root.join("four"))?,
            &b"four".to_vec(),
            "refused inventory preserved source bytes",
        )?;
        Ok(())
    }

    #[test]
    fn known_oversize_inode_stops_before_later_entry_limit() -> TestResult {
        let (_temporary, state_root, lease) = root()?;
        let lease_file =
            state_root.join(quanta_index_searchd::app::state_format::STATE_ROOT_LEASE_FILE_NAME);
        fs::write(lease_file, b"large")?;
        fs::write(state_root.join("other"), b"large")?;
        let mut byte_limit = limits();
        byte_limit.max_bytes = 4;
        byte_limit.max_entries = 1;
        let byte_error = seed(
            &lease,
            &state_root,
            byte_limit,
            &RequestBudgetV1::unbounded(),
        )
        .err()
        .ok_or_else(|| std::io::Error::other("oversize byte inventory unexpectedly succeeded"))?;
        require(
            byte_error
                .to_string()
                .contains("boot byte capacity exceeded"),
            format!(
                "first known oversized inode must refuse before reading a second entry: {byte_error}"
            ),
        )?;

        fs::write(state_root.join("third"), b"large")?;
        let mut file_limit = limits();
        file_limit.max_files = 1;
        file_limit.max_entries = 2;
        let file_error = seed(
            &lease,
            &state_root,
            file_limit,
            &RequestBudgetV1::unbounded(),
        )
        .err()
        .ok_or_else(|| std::io::Error::other("oversize file inventory unexpectedly succeeded"))?;
        require(
            file_error
                .to_string()
                .contains("boot file capacity exceeded"),
            format!("second distinct inode must refuse before reading a third entry: {file_error}"),
        )?;
        Ok(())
    }

    #[test]
    fn symlink_and_external_hardlink_refuse_without_exposing_authority() -> TestResult {
        let (temporary, state_root, lease) = root()?;
        let external = temporary.path().join("external");
        fs::write(&external, b"external")?;
        symlink(&external, state_root.join("link"))?;
        require(
            seed(&lease, &state_root, limits(), &RequestBudgetV1::unbounded()).is_err(),
            "symlink inventory was accepted",
        )?;
        fs::remove_file(state_root.join("link"))?;
        fs::hard_link(&external, state_root.join("alias"))?;
        require(
            seed(&lease, &state_root, limits(), &RequestBudgetV1::unbounded()).is_err(),
            "external hardlink inventory was accepted",
        )?;
        require_equal(
            &fs::read(&external)?,
            &b"external".to_vec(),
            "external source bytes",
        )?;
        Ok(())
    }

    #[test]
    fn cancelled_scan_and_entry_depth_frame_bounds_refuse() -> TestResult {
        let (_temporary, state_root, lease) = root()?;
        fs::create_dir(state_root.join("left"))?;
        fs::create_dir(state_root.join("right"))?;
        fs::write(state_root.join("left").join("file"), b"payload")?;
        let cancelled = RequestBudgetV1::for_duration(Duration::from_secs(60));
        cancelled.cancel_handle().cancel();
        require(
            seed(&lease, &state_root, limits(), &cancelled).is_err(),
            "cancelled inventory was accepted",
        )?;
        let mut entries = limits();
        entries.max_entries = 1;
        require(
            seed(&lease, &state_root, entries, &RequestBudgetV1::unbounded()).is_err(),
            "entry-bound inventory was accepted",
        )?;
        let mut depth = limits();
        depth.max_depth = 0;
        require(
            seed(&lease, &state_root, depth, &RequestBudgetV1::unbounded()).is_err(),
            "depth-bound inventory was accepted",
        )?;
        let mut frames = limits();
        frames.max_pending_frames = 1;
        require(
            seed(&lease, &state_root, frames, &RequestBudgetV1::unbounded()).is_err(),
            "pending-frame-bound inventory was accepted",
        )?;
        Ok(())
    }

    #[test]
    fn lease_for_different_root_refuses_before_inventory() -> TestResult {
        let (_first_temp, first_root, lease) = root()?;
        let (_second_temp, second_root, _second_lease) = root()?;
        require(
            seed(
                &lease,
                &second_root,
                limits(),
                &RequestBudgetV1::unbounded(),
            )
            .is_err(),
            "foreign root accepted another root's lease",
        )?;
        let authority = seed(&lease, &first_root, limits(), &RequestBudgetV1::unbounded())?;
        require_equal(
            &authority.snapshot()?.used_files,
            &1,
            "leased root file count",
        )?;
        Ok(())
    }

    #[test]
    fn same_path_replaced_root_cannot_seed_a_foreign_inventory() -> TestResult {
        let (temporary, state_root, lease) = root()?;
        let moved = temporary.path().join("moved-state");
        fs::rename(&state_root, &moved)?;
        fs::create_dir(&state_root)?;
        fs::set_permissions(&state_root, fs::Permissions::from_mode(0o700))?;
        fs::write(state_root.join("foreign"), b"replacement")?;
        let error = seed(&lease, &state_root, limits(), &RequestBudgetV1::unbounded())
            .err()
            .ok_or_else(|| std::io::Error::other("replaced root exposed a capacity authority"))?;
        require(
            matches!(error, CoreError::InvalidContract(_)),
            "replaced root did not fail with InvalidContract",
        )?;
        require_equal(
            &fs::read(state_root.join("foreign"))?,
            &b"replacement".to_vec(),
            "replaced root foreign bytes",
        )?;
        require(
            moved
                .join(quanta_index_searchd::app::state_format::STATE_ROOT_LEASE_FILE_NAME)
                .exists(),
            "moved root lost its lease file",
        )?;
        Ok(())
    }

    #[test]
    fn only_declared_service_socket_is_outside_regular_file_charge() -> TestResult {
        let (_temporary, state_root, lease) = root()?;
        let socket = state_root.join("ingest.sock");
        let _listener = std::os::unix::net::UnixListener::bind(&socket)?;
        require(
            seed(&lease, &state_root, limits(), &RequestBudgetV1::unbounded()).is_err(),
            "undeclared socket inventory was accepted",
        )?;
        let authority = seed_publication_capacity(
            &lease,
            &state_root,
            limits(),
            &[socket],
            &RequestBudgetV1::unbounded(),
        )?;
        require_equal(
            &authority.snapshot()?.used_files,
            &1,
            "declared socket file count",
        )?;
        Ok(())
    }
}
