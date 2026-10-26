//! Offline `backup-state`, `restore-state` and `verify-state`.
//!
//! Every operation here is offline and runs under an
//! [`OfflineSourceSessionV1`]: the session pins the source's canonical
//! identity, the custody that proves it may be read, and a
//! [`SourceFreezeReceiptV1`] taken at open. Immediately before the cutover
//! the inventory is recomputed and exact-compared with the frozen receipt;
//! any drift refuses the publish and no destination authority is created.
//! The session types live in this module rather than `state_format` (which
//! holds stateless path and manifest math with no leases) or `runtime`
//! (which owns the read-write lease): custody is the offline operation's
//! session.
//!
//! The source is never mutated: the engine acquires no lease itself (a lease
//! acquisition creates the daemon lock file), writes no lock, marker or
//! receipt into the source, and never chmods it. A current root's custody is
//! the daemon's own [`StateRootLease`], handed in by the caller — whoever
//! holds it proves exclusive ownership, and a live owner fails that handover
//! with the existing `STATE_ROOT_IN_USE` before the session opens. There is
//! no live fallback and no boot-time branch — the daemon refuses a legacy
//! root typed (see [`super::state_format::refuse_legacy_state_root_v1`]).
//!
//! The ordering rules are the whole point, so they are structural rather
//! than documented-only:
//!
//! * a destination is fully prepared in a sibling staging directory, and
//!   the root manifest is the **last** thing written there, after every data
//!   object is fsynced and the staging root has been deep-opened;
//! * the switch is one `rename` plus a parent-directory fsync, so an
//!   interruption leaves either the old root or the complete new one;
//! * a preexisting staging entry is never removed automatically, even if an
//!   earlier interruption created it; explicit inspection and cleanup are
//!   required before retry.
//!
//! Concrete adapters are named by the composition root only: this module
//! talks to [`CatalogSnapshotPort`] and [`OfflineTargetFactoryV1`].

use std::fs;
use std::path::{Path, PathBuf};

use quanta_index_contract::SearchPlaneErrorCodeV2;
use quanta_index_core::{CoreError, RequestBudgetV1};

use super::runtime::StateRootLease;
use super::state_format::{
    OfflineRootRoleV1, STAGING_DIRECTORY_SUFFIX, STATE_BACKUP_CATALOG_FILE_NAME,
    STATE_BACKUP_MANIFEST_FILE_NAME, STATE_CATALOG_DIRECTORY, STATE_ROOT_MANIFEST_FILE_NAME,
    STATE_ROOT_MANIFEST_FORMAT_VERSION, StateMigrationFaultPointV1, StateMigrationFaultPort,
    StateObjectEntryV1, StateRootFormatV1, StateRootManifestV1, detect_state_root_format_v1,
    inventory_state_directories_v1, inventory_state_root_v1, is_canonical_relative_path,
    is_sqlite_sidecar_v1, read_root_manifest_v1, refuse_broad_offline_target_v1,
    refuse_existing_destination_v1, refuse_legacy_state_root_v1, refuse_non_private_source_root_v1,
    sha256_file_hex, verify_root_against_manifest_v1,
};

/// The daemon's state-root lock file: never part of an inventory.
pub const STATE_ROOT_LOCK_FILE_NAME: &str = ".searchd-state-root.lock";

/// Exclusions for an inventory taken over a **live** root.
///
/// The lease file, any manifest, and the whole catalog directory: the live
/// catalog's bytes are not a snapshot, so the backup API carries it instead
/// and the manifest's `catalog-digest` covers its logical content.
const LIVE_ROOT_EXCLUSIONS: [&str; 4] = [
    STATE_ROOT_LOCK_FILE_NAME,
    STATE_ROOT_MANIFEST_FILE_NAME,
    STATE_BACKUP_MANIFEST_FILE_NAME,
    STATE_CATALOG_DIRECTORY,
];

/// Exclusions for an inventory taken over a **produced** root.
///
/// This includes staging and destination roots. The persistent lease is
/// runtime custody, including when deep-open created it under staging.
const PRODUCED_ROOT_EXCLUSIONS: [&str; 3] = [
    STATE_ROOT_LOCK_FILE_NAME,
    STATE_ROOT_MANIFEST_FILE_NAME,
    STATE_BACKUP_MANIFEST_FILE_NAME,
];

/// Manifest verification exclusions for a leased produced root.
///
/// The lock is runtime custody, not manifest payload;
/// unlike a live-source backup, the produced catalog remains covered by the
/// manifest and therefore must not be excluded here.
const LEASED_PRODUCED_ROOT_EXCLUSIONS: [&str; 3] = PRODUCED_ROOT_EXCLUSIONS;

fn typed(code: SearchPlaneErrorCodeV2, message: String) -> CoreError {
    CoreError::Typed { code, message }
}

fn storage(action: &str, path: &Path, error: &dyn std::fmt::Display) -> CoreError {
    CoreError::Storage(format!(
        "state migration: {action} {}: {error}",
        path.display()
    ))
}

/// One catalog's canonical logical content receipt.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CatalogSnapshotV1 {
    /// Canonical logical digest (schema plus every user table's rows).
    pub content_digest_hex: String,
    pub byte_size: u64,
    /// `(table, rows)` in schema order.
    pub table_rows: Vec<(String, u64)>,
}

impl CatalogSnapshotV1 {
    /// Total rows across every user table.
    #[must_use]
    pub fn total_rows(&self) -> u64 {
        self.table_rows.iter().map(|(_table, rows)| rows).sum()
    }
}

/// Both sides of a catalog freeze: the live catalog and the copy taken from
/// it.
///
/// The engine refuses a freeze whose halves disagree, so a snapshot is only
/// ever accepted when it is logically equal to its source.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CatalogFreezeV1 {
    pub live: CatalogSnapshotV1,
    pub snapshot: CatalogSnapshotV1,
}

impl CatalogFreezeV1 {
    /// Whether the copy is logically equal to the catalog it came from.
    #[must_use]
    pub fn is_consistent(&self) -> bool {
        self.live.content_digest_hex == self.snapshot.content_digest_hex
    }
}

/// Verify an existing source snapshot independently of destination mutation.
pub trait CatalogSnapshotPort: Send + Sync {
    /// Re-open and re-verify an existing snapshot file, without touching any
    /// other catalog.
    fn verify_snapshot_at(&self, snapshot_file: &Path) -> Result<CatalogSnapshotV1, CoreError>;
}

/// What a deep open proved about a produced root.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct StateRootDeepOpenReceiptV1 {
    /// Sealed generations the two index tracks inventoried.
    pub sealed_generations: u64,
    /// `RepoMap` candidates the catalog and object projections reconciled.
    pub repomap_candidates: u64,
    /// Catalog rows the reopen read back.
    pub catalog_rows: u64,
}

/// Begin one destination custody before its first staged object mutation.
/// The returned owner retains the original source custody and request budget.
pub trait OfflineTargetFactoryV1: Send + Sync {
    fn begin_target(
        &self,
        source: &OfflineSourceSessionV1,
        destination: &Path,
        budget: &RequestBudgetV1,
    ) -> Result<Box<dyn OfflineTargetSessionV1>, CoreError>;
}

/// All producing steps share one destination authority and its real FD owners.
///
/// Failed operations retain original close/pending custody; publication must
/// retire mutable owners and prove quiescence before renaming the leased root.
pub trait OfflineTargetSessionV1 {
    fn staging_root(&self) -> &Path;
    fn ensure_directory(&mut self, relative: &Path) -> Result<(), CoreError>;
    fn copy_object(&mut self, object: &StateObjectEntryV1) -> Result<(), CoreError>;
    fn snapshot_catalog(&mut self) -> Result<CatalogFreezeV1, CoreError>;
    fn verify_catalog(&mut self) -> Result<CatalogSnapshotV1, CoreError>;
    fn rotate_incarnation(&mut self) -> Result<(), CoreError>;
    fn deep_open(&mut self) -> Result<StateRootDeepOpenReceiptV1, CoreError>;
    fn write_manifest(
        &mut self,
        name: &str,
        manifest: &StateRootManifestV1,
        fault: &dyn StateMigrationFaultPort,
    ) -> Result<(), CoreError>;
    fn publish(&mut self) -> Result<(), CoreError>;
}

/// Which offline operation a request describes.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum OfflineStateOperationV1 {
    Backup,
    Restore,
    Verify,
}

impl OfflineStateOperationV1 {
    /// The operation's stable CLI name.
    #[must_use]
    pub const fn command_name(self) -> &'static str {
        match self {
            Self::Backup => "backup-state",
            Self::Restore => "restore-state",
            Self::Verify => "verify-state",
        }
    }
}

/// One parsed offline command, as the CLI hands it to the composition root.
///
/// `verify-state` names one root only, so `destination_root` is `None` for
/// it; the two producing operations require it and refuse the command
/// before touching anything when it is missing.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct OfflineStateCommandV1 {
    pub operation: OfflineStateOperationV1,
    pub source_root: PathBuf,
    pub destination_root: Option<PathBuf>,
}

/// What a frozen source entry is: a regular file or a directory. Anything
/// else (a symlink, a socket, a device) is refused at freeze time rather
/// than recorded.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SourceEntryKindV1 {
    File,
    Directory,
}

/// One frozen source entry.
///
/// This records the canonical relative path, the entry type, the filesystem
/// identity (device, inode, mode, owner, link count), the size, the mtime,
/// and — for regular files only — the content digest.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SourceFrozenFileV1 {
    pub relative_path: String,
    pub entry_kind: SourceEntryKindV1,
    pub device: u64,
    pub inode: u64,
    pub mode: u32,
    pub owner: u32,
    pub nlink: u64,
    pub byte_size: u64,
    pub mtime_secs: i64,
    pub mtime_nanos: u32,
    pub content_digest_hex: Option<String>,
}

/// The frozen source inventory: the canonical root, the root directory's own
/// identity, and every entry under it in `relative_path` order.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SourceFreezeReceiptV1 {
    pub canonical_root: PathBuf,
    pub root_device: u64,
    pub root_inode: u64,
    pub root_mode: u32,
    pub root_owner: u32,
    pub entries: Vec<SourceFrozenFileV1>,
}

/// Read-only custody of a source root: the canonical identity plus the root
/// directory's filesystem identity, pinned at open with read-only stats.
///
/// No file is created, no lock is taken, no mode is changed. Backup roots
/// are never live daemon roots, so identity plus the frozen
/// inventory and the pre-publish drift recheck is the whole custody.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ReadOnlyRootLeaseV1 {
    canonical_root: PathBuf,
    device: u64,
    inode: u64,
    mode: u32,
    owner: u32,
    manifest: StateRootManifestV1,
}

impl ReadOnlyRootLeaseV1 {
    /// The pinned canonical source root.
    #[must_use]
    pub fn canonical_root(&self) -> &Path {
        &self.canonical_root
    }
}

/// What proves an offline operation may read its source.
#[derive(Clone, Debug)]
pub enum OfflineSourceCustodyV1 {
    /// A current root: the daemon's own lease, handed in by the caller.
    /// Whoever holds it proves exclusive ownership.
    Current(StateRootLease),
    /// A produced backup root: read-only custody, which creates nothing
    /// inside it.
    ProducedBackup(ReadOnlyRootLeaseV1),
}

/// One offline operation's immutable-source session: the pinned canonical
/// source, the custody that proves it may be read, and the frozen inventory
/// the pre-publish drift recheck exact-compares against.
#[derive(Debug)]
pub struct OfflineSourceSessionV1 {
    canonical_root: PathBuf,
    custody: OfflineSourceCustodyV1,
    before: SourceFreezeReceiptV1,
}

impl OfflineSourceSessionV1 {
    /// Bind the daemon's own lease on a current root.
    ///
    /// The caller acquires the lease (a live owner fails that handover with
    /// the existing `STATE_ROOT_IN_USE`); this only re-proves the leased
    /// root is a private current root and freezes it, touching nothing.
    pub fn open_current(lease: StateRootLease) -> Result<Self, CoreError> {
        let canonical_root = lease.state_root_identity_v1().to_path_buf();
        refuse_non_private_source_root_v1(&canonical_root)?;
        refuse_legacy_state_root_v1(&canonical_root)?;
        require_format_v1(
            &canonical_root,
            StateRootFormatV1::CurrentV1 { manifest: false },
        )?;
        let before = freeze_source_root_v1(&canonical_root, &LIVE_ROOT_EXCLUSIONS)?;
        Ok(Self {
            canonical_root,
            custody: OfflineSourceCustodyV1::Current(lease),
            before,
        })
    }

    /// Pin a produced backup root under read-only custody and freeze it. The
    /// backup manifest must advertise first: anything else is not restorable.
    pub fn open_produced_backup(root: &Path) -> Result<Self, CoreError> {
        let lease = pin_read_only_source_v1(root)?;
        let canonical_root = lease.canonical_root().to_path_buf();
        if peek_verify_manifest_v1(&canonical_root)? != VerifyManifestKindV1::BackupRoot {
            return Err(typed(
                SearchPlaneErrorCodeV2::InvalidRequest,
                "produced-backup custody requires exactly one backup manifest".to_string(),
            ));
        }
        refuse_legacy_state_root_v1(&canonical_root)?;
        let before = freeze_source_root_v1(&canonical_root, &PRODUCED_ROOT_EXCLUSIONS)?;
        Ok(Self {
            canonical_root,
            custody: OfflineSourceCustodyV1::ProducedBackup(lease),
            before,
        })
    }

    /// The pinned canonical source root every step must read through.
    #[must_use]
    pub fn canonical_root(&self) -> &Path {
        &self.canonical_root
    }

    /// The custody that proves the source may be read.
    #[must_use]
    pub fn custody(&self) -> &OfflineSourceCustodyV1 {
        &self.custody
    }

    /// The frozen inventory the pre-publish drift recheck compares against.
    #[must_use]
    pub fn before(&self) -> &SourceFreezeReceiptV1 {
        &self.before
    }

    /// The lease a current-root operation holds, or a typed refusal when the
    /// session carries any other custody.
    pub fn require_current_lease(
        &self,
        operation: OfflineStateOperationV1,
    ) -> Result<&StateRootLease, CoreError> {
        match &self.custody {
            OfflineSourceCustodyV1::Current(lease) => Ok(lease),
            OfflineSourceCustodyV1::ProducedBackup(_) => Err(typed(
                SearchPlaneErrorCodeV2::InvalidRequest,
                format!(
                    "offline {} requires a current-root session holding the daemon lease",
                    operation.command_name(),
                ),
            )),
        }
    }

    /// The read-only lease a restore holds, or a typed refusal when the
    /// session carries any other custody.
    pub fn require_backup_lease(
        &self,
        operation: OfflineStateOperationV1,
    ) -> Result<&ReadOnlyRootLeaseV1, CoreError> {
        match &self.custody {
            OfflineSourceCustodyV1::ProducedBackup(lease) => Ok(lease),
            OfflineSourceCustodyV1::Current(_) => Err(typed(
                SearchPlaneErrorCodeV2::InvalidRequest,
                format!(
                    "offline {} requires a produced-backup read-only session",
                    operation.command_name(),
                ),
            )),
        }
    }
}

/// The exclusions the drift recheck replays for a session: exactly the set
/// the freeze used, so the comparison is exact by construction.
fn session_exclusions_v1(session: &OfflineSourceSessionV1) -> &[&str] {
    match &session.custody {
        OfflineSourceCustodyV1::Current(_) => &LIVE_ROOT_EXCLUSIONS,
        OfflineSourceCustodyV1::ProducedBackup(_) => &PRODUCED_ROOT_EXCLUSIONS,
    }
}

/// Exclusions for checking a produced manifest under the session's custody.
fn manifest_exclusions_v1(session: &OfflineSourceSessionV1) -> &[&str] {
    match &session.custody {
        OfflineSourceCustodyV1::Current(_) => &LEASED_PRODUCED_ROOT_EXCLUSIONS,
        OfflineSourceCustodyV1::ProducedBackup(_) => &PRODUCED_ROOT_EXCLUSIONS,
    }
}

/// Refuse, read-only, before any freeze: broad, non-private, or unresolvable
/// roots never become custody. Returns the pinned read-only lease.
fn pin_read_only_source_v1(root: &Path) -> Result<ReadOnlyRootLeaseV1, CoreError> {
    let admitted = refuse_broad_offline_target_v1(root, OfflineRootRoleV1::Source)?;
    refuse_non_private_source_root_v1(&admitted)?;
    let canonical_root = fs::canonicalize(&admitted)
        .map_err(|error| storage("resolve source identity", &admitted, &error))?;
    let (device, inode, mode, owner) = root_identity_v1(&canonical_root)?;
    let manifest = read_root_manifest_v1(&canonical_root.join(STATE_BACKUP_MANIFEST_FILE_NAME))?;
    Ok(ReadOnlyRootLeaseV1 {
        canonical_root,
        device,
        inode,
        mode,
        owner,
        manifest,
    })
}

/// The root directory's filesystem identity, read-only. Non-unix platforms
/// carry zeros; the entry-level comparison is the portable authority there.
fn root_identity_v1(root: &Path) -> Result<(u64, u64, u32, u32), CoreError> {
    let metadata =
        fs::symlink_metadata(root).map_err(|error| storage("inspect source root", root, &error))?;
    if !metadata.is_dir() {
        return Err(typed(
            SearchPlaneErrorCodeV2::StateRootFormatUnsupported,
            format!("source root {} is not a directory", root.display()),
        ));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt as _;
        Ok((
            metadata.dev(),
            metadata.ino(),
            metadata.mode() & 0o7777,
            metadata.uid(),
        ))
    }
    #[cfg(not(unix))]
    {
        let _ = &metadata;
        Ok((0, 0, 0, 0))
    }
}

/// Freeze the source inventory: every entry's canonical relative path, type,
/// filesystem identity, size, mtime and content digest, in path order.
///
/// Read-only: stats and content digests only. A symlink or a special file is
/// refused like the data inventory refuses it, and a hard-linked regular
/// file is refused too — a byte copy would silently un-share its links, so
/// an offline operation must not treat it as an ordinary object.
fn freeze_source_root_v1(
    root: &Path,
    exclusions: &[&str],
) -> Result<SourceFreezeReceiptV1, CoreError> {
    use std::collections::BTreeSet;
    let skip: BTreeSet<&str> = exclusions.iter().copied().collect();
    let (root_device, root_inode, root_mode, root_owner) = root_identity_v1(root)?;
    let mut entries = Vec::new();
    freeze_walk_v1(root, root, &skip, &mut entries)?;
    entries.sort_by(|left: &SourceFrozenFileV1, right: &SourceFrozenFileV1| {
        left.relative_path.cmp(&right.relative_path)
    });
    Ok(SourceFreezeReceiptV1 {
        canonical_root: root.to_path_buf(),
        root_device,
        root_inode,
        root_mode,
        root_owner,
        entries,
    })
}

fn freeze_walk_v1(
    root: &Path,
    directory: &Path,
    skip: &std::collections::BTreeSet<&str>,
    out: &mut Vec<SourceFrozenFileV1>,
) -> Result<(), CoreError> {
    let entries =
        fs::read_dir(directory).map_err(|error| storage("read directory", directory, &error))?;
    for entry in entries {
        let entry = entry.map_err(|error| storage("read directory entry", directory, &error))?;
        let path = entry.path();
        let name = entry.file_name().to_string_lossy().to_string();
        let relative = super::state_format::relative_state_path_v1(root, &path)?;
        if !relative.contains('/') && skip.contains(name.as_str()) {
            continue;
        }
        let metadata = fs::symlink_metadata(&path)
            .map_err(|error| storage("inspect object", &path, &error))?;
        if metadata.file_type().is_symlink() {
            return Err(typed(
                SearchPlaneErrorCodeV2::StateRootInsecure,
                format!(
                    "source root {} contains the symlink {relative}; an offline freeze refuses to follow it",
                    root.display()
                ),
            ));
        }
        if metadata.is_dir() {
            out.push(frozen_entry_v1(
                &relative,
                SourceEntryKindV1::Directory,
                &metadata,
                None,
            )?);
            freeze_walk_v1(root, &path, skip, out)?;
            continue;
        }
        if !metadata.is_file() {
            return Err(typed(
                SearchPlaneErrorCodeV2::StateRootInsecure,
                format!(
                    "source root {} contains the non-regular entry {relative}",
                    root.display()
                ),
            ));
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt as _;
            if metadata.nlink() != 1 {
                return Err(typed(
                    SearchPlaneErrorCodeV2::StateRootInsecure,
                    format!(
                        "source root {} contains the hard-linked file {relative} ({} links); an offline copy would silently un-share its links",
                        root.display(),
                        metadata.nlink()
                    ),
                ));
            }
        }
        if is_sqlite_sidecar_v1(&relative) {
            continue;
        }
        let (digest_hex, _byte_size) = sha256_file_hex(&path)?;
        out.push(frozen_entry_v1(
            &relative,
            SourceEntryKindV1::File,
            &metadata,
            Some(digest_hex),
        )?);
    }
    Ok(())
}

fn frozen_entry_v1(
    relative: &str,
    entry_kind: SourceEntryKindV1,
    metadata: &fs::Metadata,
    content_digest_hex: Option<String>,
) -> Result<SourceFrozenFileV1, CoreError> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt as _;
        let mtime_secs = metadata.mtime();
        let mtime_nanos = u32::try_from(metadata.mtime_nsec().max(0)).map_err(|error| {
            storage(
                "read source mtime",
                Path::new(relative),
                &format_args!("{error}"),
            )
        })?;
        Ok(SourceFrozenFileV1 {
            relative_path: relative.to_string(),
            entry_kind,
            device: metadata.dev(),
            inode: metadata.ino(),
            mode: metadata.mode() & 0o7777,
            owner: metadata.uid(),
            nlink: metadata.nlink(),
            byte_size: metadata.len(),
            mtime_secs,
            mtime_nanos,
            content_digest_hex,
        })
    }
    #[cfg(not(unix))]
    {
        let _ = metadata;
        Ok(SourceFrozenFileV1 {
            relative_path: relative.to_string(),
            entry_kind,
            device: 0,
            inode: 0,
            mode: 0,
            owner: 0,
            nlink: 0,
            byte_size: 0,
            mtime_secs: 0,
            mtime_nanos: 0,
            content_digest_hex,
        })
    }
}

/// What an offline operation produced.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct OfflineStateOutcomeV1 {
    pub operation: OfflineStateOperationV1,
    pub destination_root: PathBuf,
    pub manifest_digest_hex: String,
    pub objects: u64,
    pub catalog_digest_hex: String,
    pub catalog_rows: u64,
    pub deep_open: StateRootDeepOpenReceiptV1,
}

/// What `verify-state` proved.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct OfflineStateVerificationV1 {
    pub root: PathBuf,
    pub manifest_digest_hex: String,
    pub objects: u64,
    pub catalog_digest_hex: String,
    pub catalog_rows: u64,
}

/// Resolve and refuse an offline operation's destination before mutation.
///
/// The source already stands pinned in the session; the
/// destination must be fresh, outside the source tree (a destination inside
/// the source would mutate it), and not an alias of the source reached
/// through a symlink.
fn plan_offline_destination_v1(
    session: &OfflineSourceSessionV1,
    operation: OfflineStateOperationV1,
    destination_root: &Path,
) -> Result<PathBuf, CoreError> {
    let destination =
        refuse_broad_offline_target_v1(destination_root, OfflineRootRoleV1::Destination)?;
    refuse_existing_destination_v1(&destination, session.canonical_root())?;
    if destination == session.canonical_root() {
        return Err(typed(
            SearchPlaneErrorCodeV2::InvalidRequest,
            format!(
                "offline {} refuses a source that is its own destination ({})",
                operation.command_name(),
                destination.display()
            ),
        ));
    }
    let canonical_destination = canonicalize_through_existing_ancestor_v1(&destination)?;
    if canonical_destination == session.canonical_root()
        || canonical_destination.starts_with(session.canonical_root())
    {
        return Err(typed(
            SearchPlaneErrorCodeV2::InvalidRequest,
            format!(
                "offline {} destination {} aliases or sits inside its source {}; a destination must be outside the source tree",
                operation.command_name(),
                destination.display(),
                session.canonical_root().display()
            ),
        ));
    }
    Ok(destination)
}

/// Canonicalize a destination through its first existing ancestor, rejoining
/// whatever does not exist yet. Read-only: resolves symlinks and aliases
/// without creating anything.
fn canonicalize_through_existing_ancestor_v1(path: &Path) -> Result<PathBuf, CoreError> {
    let mut missing: Vec<std::ffi::OsString> = Vec::new();
    let mut cursor = path;
    loop {
        match fs::canonicalize(cursor) {
            Ok(mut canonical) => {
                for component in missing.iter().rev() {
                    canonical.push(component);
                }
                return Ok(canonical);
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                match (cursor.file_name(), cursor.parent()) {
                    (Some(name), Some(parent)) => {
                        missing.push(name.to_os_string());
                        cursor = parent;
                    }
                    _ => {
                        return Err(storage("resolve destination identity", path, &error));
                    }
                }
            }
            Err(error) => {
                return Err(storage("resolve destination identity", cursor, &error));
            }
        }
    }
}

/// Exact-compare the source inventory with its frozen receipt before publish.
///
/// On drift no destination authority is created. Leave staging for explicit
/// inspection and cleanup: path-based
/// recursive deletion cannot prove that the staging name still denotes the
/// directory this attempt created.
fn refuse_source_drift_v1(
    session: &OfflineSourceSessionV1,
    operation: OfflineStateOperationV1,
    staging: &Path,
) -> Result<(), CoreError> {
    let drift = match source_matches_freeze_v1(session) {
        Ok(true) => return Ok(()),
        Ok(false) => typed(
            SearchPlaneErrorCodeV2::StateRootInsecure,
            format!(
                "offline {} source {} changed during the operation; the frozen inventory no longer matches, so nothing is published; inspect staging {} before an explicit cleanup",
                operation.command_name(),
                session.canonical_root().display(),
                staging.display()
            ),
        ),
        // A missing, malformed or unreadable source authority is drift too;
        // preserve its original error and leave staging for inspection.
        Err(error) => error,
    };
    Err(drift)
}

fn source_matches_freeze_v1(session: &OfflineSourceSessionV1) -> Result<bool, CoreError> {
    if let OfflineSourceCustodyV1::ProducedBackup(lease) = session.custody()
        && (peek_verify_manifest_v1(session.canonical_root())? != VerifyManifestKindV1::BackupRoot
            || read_root_manifest_v1(
                &session
                    .canonical_root()
                    .join(STATE_BACKUP_MANIFEST_FILE_NAME),
            )? != lease.manifest)
    {
        return Ok(false);
    }
    let observed = freeze_source_root_v1(session.canonical_root(), session_exclusions_v1(session))?;
    Ok(match session.custody() {
        // Restore now verifies the read-only SQLite snapshot before copying.
        // Reuse verification's exact, bounded sidecar-directory exception;
        // every advertised file and non-catalog directory remains exact.
        OfflineSourceCustodyV1::ProducedBackup(_) => {
            same_verification_freeze(session.before(), &observed)
        }
        OfflineSourceCustodyV1::Current(_) => observed == *session.before(),
    })
}

fn same_frozen_root_identity(
    before: &SourceFreezeReceiptV1,
    after: &SourceFreezeReceiptV1,
) -> bool {
    before.canonical_root == after.canonical_root
        && before.root_device == after.root_device
        && before.root_inode == after.root_inode
        && before.root_mode == after.root_mode
        && before.root_owner == after.root_owner
}

/// Compare verification freezes with a bounded catalog-directory exception.
///
/// `SQLite` read-only WAL inspection can create/remove excluded WAL/SHM
/// sidecars. Those change the catalog directory's size, mtime and APFS link
/// count, not its custody or advertised payload. Compare its identity while
/// retaining exact file bytes/metadata and all other directory metadata.
fn same_verification_freeze(before: &SourceFreezeReceiptV1, after: &SourceFreezeReceiptV1) -> bool {
    same_frozen_root_identity(before, after)
        && before.entries.len() == after.entries.len()
        && before
            .entries
            .iter()
            .zip(&after.entries)
            .all(|(left, right)| {
                if left.relative_path == STATE_CATALOG_DIRECTORY
                    && left.entry_kind == SourceEntryKindV1::Directory
                {
                    left.relative_path == right.relative_path
                        && left.entry_kind == right.entry_kind
                        && left.device == right.device
                        && left.inode == right.inode
                        && left.mode == right.mode
                        && left.owner == right.owner
                        && left.content_digest_hex == right.content_digest_hex
                } else {
                    left == right
                }
            })
}

/// Require the current root format for backup custody.
fn require_format_v1(source: &Path, expected: StateRootFormatV1) -> Result<(), CoreError> {
    let observed = detect_state_root_format_v1(source)?;
    let matches = matches!(
        (observed, expected),
        (
            StateRootFormatV1::CurrentV1 { .. },
            StateRootFormatV1::CurrentV1 { .. }
        ) | (StateRootFormatV1::Absent, StateRootFormatV1::Absent)
    );
    if matches {
        return Ok(());
    }
    Err(typed(
        SearchPlaneErrorCodeV2::StateRootFormatUnsupported,
        format!(
            "offline state source root {} is {observed:?}; this operation requires {expected:?}",
            source.display()
        ),
    ))
}

/// Copy only the frozen inventory through the single destination authority.
fn copy_data_objects_v1(
    target: &mut dyn OfflineTargetSessionV1,
    directories: &[String],
    objects: &[StateObjectEntryV1],
) -> Result<(), CoreError> {
    for directory in directories {
        target.ensure_directory(Path::new(directory))?;
    }
    for object in objects {
        if !is_canonical_relative_path(&object.relative_path) {
            return Err(typed(
                SearchPlaneErrorCodeV2::StateRootInsecure,
                format!(
                    "source inventory named non-canonical path {}",
                    object.relative_path
                ),
            ));
        }
        target.copy_object(object)?;
    }
    Ok(())
}

/// The one place a produced root's manifest is assembled and written last.
fn publish_staging_manifest_v1(
    target: &mut dyn OfflineTargetSessionV1,
    manifest_file_name: &str,
    catalog: &CatalogSnapshotV1,
    fault: &dyn StateMigrationFaultPort,
) -> Result<StateRootManifestV1, CoreError> {
    fault.reach(StateMigrationFaultPointV1::AfterDataSync)?;
    let staging = target.staging_root().to_path_buf();
    let objects = inventory_state_root_v1(&staging, &PRODUCED_ROOT_EXCLUSIONS)?;
    let directories = inventory_state_directories_v1(&staging, &PRODUCED_ROOT_EXCLUSIONS)?;
    let manifest = StateRootManifestV1 {
        format_version: STATE_ROOT_MANIFEST_FORMAT_VERSION,
        root_format: StateRootFormatV1::CurrentV1 { manifest: true },
        catalog_digest_hex: catalog.content_digest_hex.clone(),
        catalog_rows: catalog.total_rows(),
        objects,
        directories,
    };
    target.write_manifest(manifest_file_name, &manifest, fault)?;
    Ok(manifest)
}

fn finish_offline_operation_v1(
    operation: OfflineStateOperationV1,
    target: &mut dyn OfflineTargetSessionV1,
    destination: &Path,
    manifest_file_name: &str,
    deep_open: StateRootDeepOpenReceiptV1,
    fault: &dyn StateMigrationFaultPort,
) -> Result<OfflineStateOutcomeV1, CoreError> {
    target.publish()?;
    fault.reach(StateMigrationFaultPointV1::AfterCutoverRename)?;
    let published = read_root_manifest_v1(&destination.join(manifest_file_name))?;
    verify_root_against_manifest_v1(
        destination,
        manifest_file_name,
        &published,
        &PRODUCED_ROOT_EXCLUSIONS,
    )?;
    Ok(OfflineStateOutcomeV1 {
        operation,
        destination_root: destination.to_path_buf(),
        manifest_digest_hex: published.manifest_digest_hex(),
        objects: u64::try_from(published.objects.len()).map_or(u64::MAX, |count| count),
        catalog_digest_hex: published.catalog_digest_hex.clone(),
        catalog_rows: published.catalog_rows,
        deep_open,
    })
}

/// `backup-state`: freeze a current root held in `session` into a new
/// directory whose every object is enumerated, digested and manifest-last.
///
/// The session must carry [`OfflineSourceCustodyV1::Current`]: the daemon's
/// own lease, handed in by the caller. The source is read through the
/// session's canonical root only, and the frozen inventory is re-proved
/// immediately before the cutover.
pub fn run_offline_backup_v1(
    session: &OfflineSourceSessionV1,
    destination_root: &Path,
    targets: &dyn OfflineTargetFactoryV1,
    fault: &dyn StateMigrationFaultPort,
    budget: &RequestBudgetV1,
) -> Result<OfflineStateOutcomeV1, CoreError> {
    const OPERATION: OfflineStateOperationV1 = OfflineStateOperationV1::Backup;
    budget.checkpoint("offline backup admission")?;
    let _lease = session.require_current_lease(OPERATION)?;
    let source = session.canonical_root();
    let destination = plan_offline_destination_v1(session, OPERATION, destination_root)?;
    refuse_legacy_state_root_v1(source)?;
    require_format_v1(source, StateRootFormatV1::CurrentV1 { manifest: false })?;
    let objects = inventory_state_root_v1(source, &LIVE_ROOT_EXCLUSIONS)?;
    let directories = inventory_state_directories_v1(source, &LIVE_ROOT_EXCLUSIONS)?;
    refuse_source_drift_v1(session, OPERATION, &destination)?;
    let mut target = targets.begin_target(session, &destination, budget)?;
    let staging = target.staging_root().to_path_buf();
    copy_data_objects_v1(target.as_mut(), &directories, &objects)?;
    target.ensure_directory(Path::new(STATE_CATALOG_DIRECTORY))?;
    let freeze = target.snapshot_catalog()?;
    if !freeze.is_consistent() {
        return Err(typed(
            SearchPlaneErrorCodeV2::CatalogRowCorrupt,
            format!(
                "catalog snapshot of {} digests to {} but the live catalog digests to {}; the snapshot is not a freeze of its source",
                source.display(),
                freeze.snapshot.content_digest_hex,
                freeze.live.content_digest_hex
            ),
        ));
    }

    // A backup root must be a root the daemon could open, so its staged copy
    // is deep-opened exactly as a restored one is: the freeze
    // boundary then covers a reconciled root. Restore preserves its logical
    // Catalog identity; SQLite journal normalization may update only physical
    // header counters. The catalog receipt is taken again
    // afterwards, because reconciliation is a real mutation and the manifest
    // must describe the root it actually publishes.
    let deep = target.deep_open()?;
    let settled = target.verify_catalog()?;
    let _manifest = publish_staging_manifest_v1(
        target.as_mut(),
        STATE_BACKUP_MANIFEST_FILE_NAME,
        &settled,
        fault,
    )?;
    refuse_source_drift_v1(session, OPERATION, &staging)?;
    budget.checkpoint("offline backup before cutover")?;
    finish_offline_operation_v1(
        OPERATION,
        target.as_mut(),
        &destination,
        STATE_BACKUP_MANIFEST_FILE_NAME,
        deep,
        fault,
    )
}

/// `restore-state`: rebuild a root from a backup directory held in
/// `session`, verifying the backup manifest, re-proving the catalog digest,
/// deep-opening the staged root, and cutting over atomically.
///
/// The session must carry [`OfflineSourceCustodyV1::ProducedBackup`]:
/// read-only custody that creates nothing inside the backup root.
pub fn run_offline_restore_v1(
    session: &OfflineSourceSessionV1,
    destination_root: &Path,
    catalog: &dyn CatalogSnapshotPort,
    targets: &dyn OfflineTargetFactoryV1,
    fault: &dyn StateMigrationFaultPort,
    budget: &RequestBudgetV1,
) -> Result<OfflineStateOutcomeV1, CoreError> {
    const OPERATION: OfflineStateOperationV1 = OfflineStateOperationV1::Restore;
    budget.checkpoint("offline restore admission")?;
    let lease = session.require_backup_lease(OPERATION)?;
    let destination = plan_offline_destination_v1(session, OPERATION, destination_root)?;
    // Admit the original authority before creating staging, using the same
    // inventory, catalog and custody contract as operator verification.
    let _verified = run_offline_verify_v1(session, catalog)?;
    let backup_manifest = &lease.manifest;
    refuse_source_drift_v1(session, OPERATION, &destination)?;
    let mut target = targets.begin_target(session, &destination, budget)?;
    let staging = target.staging_root().to_path_buf();
    copy_data_objects_v1(
        target.as_mut(),
        &backup_manifest.directories,
        &backup_manifest.objects,
    )?;
    // Prove the copied bytes against the admitted authority before applying
    // activation rotation and the staged Catalog's journal normalization.
    verify_root_against_manifest_v1(
        &staging,
        STATE_BACKUP_MANIFEST_FILE_NAME,
        backup_manifest,
        &PRODUCED_ROOT_EXCLUSIONS,
    )?;
    target.rotate_incarnation()?;
    let receipt = target.verify_catalog()?;
    verify_manifest_catalog_v1(&staging, backup_manifest, &receipt)?;
    let deep = target.deep_open()?;
    let settled = target.verify_catalog()?;
    let _staged_manifest = publish_staging_manifest_v1(
        target.as_mut(),
        STATE_ROOT_MANIFEST_FILE_NAME,
        &settled,
        fault,
    )?;
    refuse_source_drift_v1(session, OPERATION, &staging)?;
    budget.checkpoint("offline restore before cutover")?;
    finish_offline_operation_v1(
        OPERATION,
        target.as_mut(),
        &destination,
        STATE_ROOT_MANIFEST_FILE_NAME,
        deep,
        fault,
    )
}

/// Which manifest a `verify-state` target advertises, peeked read-only
/// before any session opens so the composition root routes custody without
/// creating anything.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum VerifyManifestKindV1 {
    /// A produced state root: verified under the daemon-lease session.
    CurrentRoot,
    /// A backup root: verified under the read-only backup session.
    BackupRoot,
}

/// Peek which manifest `root` advertises: both is ambiguous
/// (`InvalidRequest`), neither is not verifiable (`NotFound`). Read-only.
pub fn peek_verify_manifest_v1(root: &Path) -> Result<VerifyManifestKindV1, CoreError> {
    let root_manifest = root.join(STATE_ROOT_MANIFEST_FILE_NAME);
    let backup_manifest = root.join(STATE_BACKUP_MANIFEST_FILE_NAME);
    // Reserved authority entries are present even when dangling or non-regular.
    // `is_file` follows links and collapses unreadable/unsafe authority to absent.
    // Decode the selected authority later through the canonical no-follow reader.
    let advertised = |path: &Path| match fs::symlink_metadata(path) {
        Ok(_) => Ok(true),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(storage("inspect advertised manifest", path, &error)),
    };
    match (advertised(&root_manifest)?, advertised(&backup_manifest)?) {
        (true, true) => Err(typed(
            SearchPlaneErrorCodeV2::InvalidRequest,
            format!(
                "root {} advertises both {} and {}; which manifest describes it is ambiguous",
                root.display(),
                STATE_ROOT_MANIFEST_FILE_NAME,
                STATE_BACKUP_MANIFEST_FILE_NAME
            ),
        )),
        (true, false) => Ok(VerifyManifestKindV1::CurrentRoot),
        (false, true) => Ok(VerifyManifestKindV1::BackupRoot),
        (false, false) => Err(typed(
            SearchPlaneErrorCodeV2::NotFound,
            format!(
                "root {} advertises no {STATE_ROOT_MANIFEST_FILE_NAME} and no {STATE_BACKUP_MANIFEST_FILE_NAME}",
                root.display()
            ),
        )),
    }
}

/// `verify-state`: re-prove a produced root held in `session` against its
/// own manifest and the catalog the manifest records.
///
/// The session must carry current or backup custody: a current root's
/// verification requires the daemon lease (a live owner fails that handover
/// with the existing `STATE_ROOT_IN_USE`), a backup root verifies under
/// read-only custody.
pub fn run_offline_verify_v1(
    session: &OfflineSourceSessionV1,
    catalog: &dyn CatalogSnapshotPort,
) -> Result<OfflineStateVerificationV1, CoreError> {
    let root = session.canonical_root().to_path_buf();
    // Two artifact kinds carry a manifest — a produced state root and a
    // backup root — and a root that advertises both is ambiguous, so it is
    // refused rather than resolved. The peek is re-run here so the engine
    // never trusts the composition root's routing.
    let manifest_kind = peek_verify_manifest_v1(&root)?;
    let (manifest_file_name, manifest_path) = match manifest_kind {
        VerifyManifestKindV1::CurrentRoot => (
            STATE_ROOT_MANIFEST_FILE_NAME,
            root.join(STATE_ROOT_MANIFEST_FILE_NAME),
        ),
        VerifyManifestKindV1::BackupRoot => (
            STATE_BACKUP_MANIFEST_FILE_NAME,
            root.join(STATE_BACKUP_MANIFEST_FILE_NAME),
        ),
    };
    let manifest = read_root_manifest_v1(&manifest_path)?;
    if let OfflineSourceCustodyV1::ProducedBackup(lease) = session.custody()
        && (manifest_kind != VerifyManifestKindV1::BackupRoot || manifest != lease.manifest)
    {
        return Err(typed(
            SearchPlaneErrorCodeV2::StateRootInsecure,
            "backup manifest changed after custody opened".to_string(),
        ));
    }
    if manifest.root_format != (StateRootFormatV1::CurrentV1 { manifest: true }) {
        return Err(typed(
            SearchPlaneErrorCodeV2::StateRootFormatUnsupported,
            format!(
                "root {} advertises root-format {:?} in {}",
                root.display(),
                manifest.root_format,
                manifest_path.display()
            ),
        ));
    }
    // A current session's freeze is for backup and deliberately excludes
    // the live catalog. Verification is over a produced root and must bind
    // the catalog bytes too, just as backup custody already does. Reuse the
    // existing backup freeze; take a verification-scoped one only for a
    // current-root lease.
    let verification_before = match session.custody() {
        OfflineSourceCustodyV1::Current(_) => {
            let frozen = freeze_source_root_v1(&root, manifest_exclusions_v1(session))?;
            // Extending the backup freeze with catalog bytes must not reset
            // its already-pinned non-catalog identities. Compare the same
            // entries without another filesystem walk.
            if !session
                .before()
                .entries
                .iter()
                .eq(frozen.entries.iter().filter(|entry| {
                    entry.relative_path.split('/').next() != Some(STATE_CATALOG_DIRECTORY)
                }))
            {
                return Err(typed(
                    SearchPlaneErrorCodeV2::StateRootInsecure,
                    format!(
                        "offline verify-state source {} changed after custody opened",
                        root.display()
                    ),
                ));
            }
            frozen
        }
        OfflineSourceCustodyV1::ProducedBackup(_) => session.before().clone(),
    };
    if !same_frozen_root_identity(session.before(), &verification_before) {
        return Err(typed(
            SearchPlaneErrorCodeV2::StateRootInsecure,
            format!(
                "offline verify-state source {} no longer matches its custody",
                root.display()
            ),
        ));
    }
    verify_root_against_manifest_v1(
        &root,
        manifest_file_name,
        &manifest,
        manifest_exclusions_v1(session),
    )?;
    let snapshot_path = root
        .join(STATE_CATALOG_DIRECTORY)
        .join(STATE_BACKUP_CATALOG_FILE_NAME);
    let receipt = catalog.verify_snapshot_at(&snapshot_path)?;
    verify_manifest_catalog_v1(&root, &manifest, &receipt)?;
    // A produced backup has read-only custody, not an exclusive lock. A
    // catalog verifier (or another local writer) may change the source after
    // the object walk; neither a previous object digest nor the session's
    // initial freeze proves the returned result still describes this root.
    let verification_after = freeze_source_root_v1(&root, manifest_exclusions_v1(session))?;
    if !same_verification_freeze(&verification_before, &verification_after) {
        return Err(typed(
            SearchPlaneErrorCodeV2::StateRootInsecure,
            format!(
                "offline verify-state source {} changed during verification",
                root.display()
            ),
        ));
    }
    // Manifest files are intentionally excluded from the object inventory.
    // Re-read the selected authority separately so a concurrent replacement
    // cannot turn a verification of one manifest into a success for another.
    if read_root_manifest_v1(&manifest_path)? != manifest {
        return Err(typed(
            SearchPlaneErrorCodeV2::StateRootInsecure,
            format!(
                "offline verify-state manifest {} changed during verification",
                manifest_path.display()
            ),
        ));
    }
    if peek_verify_manifest_v1(&root)? != manifest_kind {
        return Err(typed(
            SearchPlaneErrorCodeV2::StateRootInsecure,
            format!(
                "offline verify-state manifest kind in {} changed during verification",
                root.display()
            ),
        ));
    }
    Ok(OfflineStateVerificationV1 {
        root,
        manifest_digest_hex: manifest.manifest_digest_hex(),
        objects: u64::try_from(manifest.objects.len()).map_or(u64::MAX, |count| count),
        catalog_digest_hex: manifest.catalog_digest_hex.clone(),
        catalog_rows: manifest.catalog_rows,
    })
}

fn verify_manifest_catalog_v1(
    root: &Path,
    manifest: &StateRootManifestV1,
    receipt: &CatalogSnapshotV1,
) -> Result<(), CoreError> {
    if receipt.content_digest_hex != manifest.catalog_digest_hex {
        return Err(typed(
            SearchPlaneErrorCodeV2::CatalogRowCorrupt,
            format!(
                "root {} catalog digests to {} but its manifest records {}",
                root.display(),
                receipt.content_digest_hex,
                manifest.catalog_digest_hex
            ),
        ));
    }
    let expected_rows = manifest.catalog_rows;
    let observed_rows = receipt.total_rows();
    if expected_rows != observed_rows {
        return Err(typed(
            SearchPlaneErrorCodeV2::CatalogRowCorrupt,
            format!(
                "root {} catalog holds {observed_rows} rows but its manifest records {expected_rows}",
                root.display()
            ),
        ));
    }
    Ok(())
}

/// The staging suffix, re-exported so an operator-facing message and the
/// cleanup helper agree on one name.
#[must_use]
pub const fn staging_suffix_v1() -> &'static str {
    STAGING_DIRECTORY_SUFFIX
}
