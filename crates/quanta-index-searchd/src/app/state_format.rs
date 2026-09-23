//! Persistent state-root format identity, the root manifest, and the
//! offline-only refusal/cutover primitives (SEP-21 P10 / S21-11).
//!
//! Three things live here and nothing else:
//!
//! * **Format identity.** [`detect_state_root_format_v1`] answers what a
//!   directory is: a legacy layout this build must not open, a current root
//!   optionally carrying a root manifest, or nothing at all. Production boot
//!   refuses a legacy directory typed
//!   ([`SearchPlaneErrorCodeV2::StateRootFormatUnsupported`]) instead of
//!   migrating it: migration is an explicit offline operation, so the hot
//!   path never carries a legacy decoder.
//! * **The root manifest.** A canonical, self-digesting text manifest that
//!   enumerates every data object of a root by relative path, byte size and
//!   SHA-256, plus the catalog's content digest and row count. It is written
//!   *last* — after every data object is durable — so a crash can never
//!   advertise a manifest whose data is missing.
//! * **Refusal before mutation.** Broad, unresolved, aliasing, symlinked,
//!   non-private, foreign-owned and live-leased targets are refused typed
//!   before any byte is written, and the cutover itself is one `rename` so an
//!   interrupted switch leaves either the old root or the new one.

use std::collections::BTreeSet;
use std::fs;
use std::path::{Component, Path, PathBuf};

use quanta_index_contract::SearchPlaneErrorCodeV2;
use quanta_index_core::CoreError;
use sha2::{Digest as _, Sha256};

/// Version of the canonical state-root manifest
/// (`state-root-manifest-v1.txt`, and the identical backup manifest at the
/// top of a `state-backup` root).
pub const STATE_ROOT_MANIFEST_FORMAT_VERSION: u32 = 1;

/// Version of the migration receipt an offline `migrate-state` leaves in the
/// produced root.
pub const STATE_MIGRATION_RECEIPT_FORMAT_VERSION: u32 = 1;

/// The root manifest's file name, at the top of a current state root.
pub const STATE_ROOT_MANIFEST_FILE_NAME: &str = "state-root-manifest-v1.txt";

/// The backup manifest's file name, at the top of a `backup-state` root.
pub const STATE_BACKUP_MANIFEST_FILE_NAME: &str = "state-backup-manifest-v1.txt";

/// The migration receipt's file name, at the top of a migrated root.
pub const STATE_MIGRATION_RECEIPT_FILE_NAME: &str = "state-migration-receipt-v1.txt";

/// The catalog directory and file under a state root.
pub const STATE_CATALOG_DIRECTORY: &str = "catalog";

/// The catalog snapshot's file name inside a backup root's catalog
/// directory.
pub const STATE_BACKUP_CATALOG_FILE_NAME: &str = "catalog-v1.sqlite";

/// Persistent lease file; the OS lock, not its presence, owns liveness.
pub const STATE_ROOT_LEASE_FILE_NAME: &str = ".searchd-state-root.lock";

/// The only directory an offline operation is allowed to create beside a
/// destination root while it prepares the switch.
pub const STAGING_DIRECTORY_SUFFIX: &str = ".p10-staging";

/// Legacy semantic journal, relative to a state root: migration input only.
pub const LEGACY_SEMANTIC_JOURNAL_RELATIVE: &str = "semantic/journal.cbor";

/// Pre-catalog auxiliary snapshots are legacy input, never a boot-time
/// migration source. The offline importer must account for each one before
/// publishing a current root.
pub const LEGACY_AUXILIARY_SNAPSHOT_RELATIVES: [&str; 3] = [
    "authorities/history/state.cbor",
    "authorities/runtime/state.cbor",
    "authorities/structural/state.cbor",
];

/// Legacy `RepoMap` layout directories, relative to `state_root/repo-map`.
pub const LEGACY_REPOMAP_DIRECTORY_NAMES: [&str; 2] = ["activations", "snapshots"];

/// Where the `RepoMap` generation store keeps its objects.
pub const REPOMAP_DIRECTORY: &str = "repo-map";

/// What a directory under the configured state-root path actually is.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StateRootFormatV1 {
    /// No entries at all: a fresh root this build may create in place.
    Absent,
    /// A current-format root. `manifest` says whether it advertises one.
    CurrentV1 { manifest: bool },
    /// A legacy layout. Offline `migrate-state` owns it; boot refuses it.
    LegacyV1,
}

/// Directories a state root that already holds data but no manifest can
/// still be recognised by: they are written by current-format adapters.
const CURRENT_V1_MARKER_DIRECTORIES: [&str; 5] = [
    "catalog",
    "indexes",
    "repo-map",
    "authorities",
    "activations",
];

fn typed(code: SearchPlaneErrorCodeV2, message: String) -> CoreError {
    CoreError::Typed { code, message }
}

fn storage(action: &str, path: &Path, error: &dyn std::fmt::Display) -> CoreError {
    CoreError::Storage(format!(
        "state migration: {action} {}: {error}",
        path.display()
    ))
}

/// Which legacy artifacts `root` carries, as relative paths.
#[must_use]
pub fn legacy_state_root_markers_v1(root: &Path) -> Vec<String> {
    let mut markers = Vec::new();
    let journal = root.join(LEGACY_SEMANTIC_JOURNAL_RELATIVE);
    if journal.exists() {
        markers.push(LEGACY_SEMANTIC_JOURNAL_RELATIVE.to_string());
    }
    for relative in LEGACY_AUXILIARY_SNAPSHOT_RELATIVES {
        // Include dangling symlinks: they are not an absent snapshot and
        // must not let a mixed-format root be classified as current. An
        // unreadable path is also not evidence that the snapshot is absent.
        if !matches!(
            fs::symlink_metadata(root.join(relative)),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound
        ) {
            markers.push(relative.to_string());
        }
    }
    let repomap_root = root.join(REPOMAP_DIRECTORY);
    for name in LEGACY_REPOMAP_DIRECTORY_NAMES {
        if repomap_root.join(name).is_dir() {
            markers.push(format!("{REPOMAP_DIRECTORY}/{name}"));
        }
    }
    markers
}

/// What `root` is, by content: an absent root, a legacy layout, or a current
/// root that may or may not advertise its manifest.
///
/// A root that holds data but no manifest is `CurrentV1 { manifest: false }`:
/// the manifest is produced by the offline operations, so a root the daemon
/// built itself is a legitimate current root. A root carrying a *legacy*
/// marker is `LegacyV1` regardless of what else it holds — the presence of
/// one legacy artifact means a legacy binary wrote it, and a mixture must be
/// imported, never opened.
pub fn detect_state_root_format_v1(root: &Path) -> Result<StateRootFormatV1, CoreError> {
    if !root.exists() {
        return Ok(StateRootFormatV1::Absent);
    }
    if !root.is_dir() {
        return Err(typed(
            SearchPlaneErrorCodeV2::StateRootFormatUnsupported,
            format!("state root {} is not a directory", root.display()),
        ));
    }
    if !legacy_state_root_markers_v1(root).is_empty() {
        return Ok(StateRootFormatV1::LegacyV1);
    }
    let entries = fs::read_dir(root).map_err(|error| storage("read root", root, &error))?;
    let mut any = false;
    for entry in entries {
        let entry = entry.map_err(|error| storage("read root entry", root, &error))?;
        if entry.file_name() != STATE_ROOT_LEASE_FILE_NAME {
            any = true;
        }
    }
    if !any {
        return Ok(StateRootFormatV1::Absent);
    }
    let manifest = root.join(STATE_ROOT_MANIFEST_FILE_NAME).is_file();
    let known = CURRENT_V1_MARKER_DIRECTORIES
        .iter()
        .any(|name| root.join(name).is_dir());
    if !known && !manifest {
        return Ok(StateRootFormatV1::LegacyV1);
    }
    Ok(StateRootFormatV1::CurrentV1 { manifest })
}

/// Refuse a legacy state root typed, before anything opens it.
///
/// The daemon's boot path calls this; there is deliberately no migration
/// branch here. An operator with a legacy root runs the offline
/// `migrate-state` command against it and boots the produced root.
pub fn refuse_legacy_state_root_v1(root: &Path) -> Result<(), CoreError> {
    let format = detect_state_root_format_v1(root)?;
    if format != StateRootFormatV1::LegacyV1 {
        return Ok(());
    }
    Err(typed(
        SearchPlaneErrorCodeV2::StateRootFormatUnsupported,
        format!(
            "state root {} carries a legacy layout ({}); this build does not migrate at boot. \
             Run the offline `migrate-state` command and boot the produced root",
            root.display(),
            legacy_state_root_markers_v1(root).join(", ")
        ),
    ))
}

/// One enumerated data object: relative path, byte size, SHA-256 hex.
#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd)]
pub struct StateObjectEntryV1 {
    pub relative_path: String,
    pub byte_size: u64,
    pub digest_hex: String,
}

/// The canonical root manifest: format version, what kind of root it
/// describes, the catalog's logical content digest, and every data object.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StateRootManifestV1 {
    pub format_version: u32,
    pub root_format: StateRootFormatV1,
    /// Canonical content digest of the catalog snapshot (schema + every user
    /// table's rows). The restored active identities, terminal receipts,
    /// replay floor and sequence high-water all live in those rows, so this
    /// one digest is what proves they match.
    pub catalog_digest_hex: String,
    /// Total rows across the catalog's user tables.
    pub catalog_rows: u64,
    /// Objects, sorted by `relative_path`, digest of the manifest body
    /// excluded from `manifest_digest_hex` by construction.
    pub objects: Vec<StateObjectEntryV1>,
    /// Every directory under the root, sorted. Directories carry no digest,
    /// but an empty one is state a restore must reproduce: a vendor open
    /// that fsyncs an object's parent directory needs that directory to
    /// exist even before the first object is written into it.
    pub directories: Vec<String>,
}

/// The tag a manifest writes for the root it describes.
///
/// A manifest exists only inside a produced current root, so both
/// `CurrentV1` states carry the same tag: the `manifest` flag distinguishes
/// "advertises a manifest" on disk, which a manifest cannot describe about
/// itself.
fn format_tag(format: StateRootFormatV1) -> &'static str {
    match format {
        StateRootFormatV1::Absent => "absent",
        StateRootFormatV1::CurrentV1 { manifest: _ } => "current-v1",
        StateRootFormatV1::LegacyV1 => "legacy-v1",
    }
}

impl StateRootManifestV1 {
    /// The manifest body: every line except `root-digest`, each terminated by
    /// `\n`. This is the exact byte string the digest covers.
    #[must_use]
    pub fn body(&self) -> String {
        let mut lines: Vec<String> = vec![
            "quanta-index-state-root-manifest".to_string(),
            format!("format-version {}", self.format_version),
            format!("root-format {}", format_tag(self.root_format)),
            format!("catalog-digest {}", self.catalog_digest_hex),
            format!("catalog-rows {}", self.catalog_rows),
        ];
        let mut objects = self.objects.clone();
        objects.sort();
        for object in &objects {
            lines.push(format!(
                "object {} {} {}",
                object.digest_hex, object.byte_size, object.relative_path
            ));
        }
        let mut directories = self.directories.clone();
        directories.sort();
        directories.dedup();
        for directory in &directories {
            lines.push(format!("dir {directory}"));
        }
        let mut body = lines.join("\n");
        body.push('\n');
        body
    }

    /// Hex SHA-256 over [`Self::body`].
    #[must_use]
    pub fn manifest_digest_hex(&self) -> String {
        hex_lower(&Sha256::digest(self.body().as_bytes()))
    }

    /// The canonical on-disk bytes: the body, then the self-digest line.
    #[must_use]
    pub fn encode(&self) -> String {
        let mut encoded = self.body();
        encoded.push_str("root-digest ");
        encoded.push_str(&self.manifest_digest_hex());
        encoded.push('\n');
        encoded
    }

    /// Strict decode: every line must be recognised, the field set exact,
    /// the objects sorted and unique, and the trailing `root-digest` must
    /// equal the digest recomputed over the preceding body.
    pub fn decode(bytes: &str) -> Result<Self, CoreError> {
        let refuse = |detail: String| {
            typed(
                SearchPlaneErrorCodeV2::StateRootFormatUnsupported,
                format!("state-root manifest: {detail}"),
            )
        };
        let mut lines = bytes.lines();
        if lines.next() != Some("quanta-index-state-root-manifest") {
            return Err(refuse("missing manifest banner".to_string()));
        }
        let mut format_version: Option<u32> = None;
        let mut root_format: Option<StateRootFormatV1> = None;
        let mut catalog_digest_hex: Option<String> = None;
        let mut catalog_rows: Option<u64> = None;
        let mut objects: Vec<StateObjectEntryV1> = Vec::new();
        let mut directories: Vec<String> = Vec::new();
        let mut body = String::new();
        body.push_str("quanta-index-state-root-manifest\n");
        let mut declared_digest: Option<String> = None;
        for line in lines {
            if let Some(rest) = line.strip_prefix("root-digest ") {
                if declared_digest.is_some() {
                    return Err(refuse("duplicate root-digest line".to_string()));
                }
                declared_digest = Some(rest.to_string());
                continue;
            }
            if declared_digest.is_some() {
                return Err(refuse("content after root-digest".to_string()));
            }
            body.push_str(line);
            body.push('\n');
            if let Some(rest) = line.strip_prefix("format-version ") {
                format_version = Some(rest.parse::<u32>().map_err(|error| {
                    refuse(format!("format-version {rest} is not a number: {error}"))
                })?);
            } else if let Some(rest) = line.strip_prefix("root-format ") {
                root_format = Some(match rest {
                    "current-v1" => StateRootFormatV1::CurrentV1 { manifest: true },
                    "legacy-v1" => StateRootFormatV1::LegacyV1,
                    "absent" => StateRootFormatV1::Absent,
                    other => return Err(refuse(format!("unknown root-format {other}"))),
                });
            } else if let Some(rest) = line.strip_prefix("catalog-digest ") {
                catalog_digest_hex = Some(rest.to_string());
            } else if let Some(rest) = line.strip_prefix("catalog-rows ") {
                catalog_rows = Some(rest.parse::<u64>().map_err(|error| {
                    refuse(format!("catalog-rows {rest} is not a number: {error}"))
                })?);
            } else if let Some(rest) = line.strip_prefix("dir ") {
                if !is_canonical_relative_path(rest) {
                    return Err(refuse(format!(
                        "directory path {rest} is not a canonical relative path"
                    )));
                }
                directories.push(rest.to_string());
            } else if let Some(rest) = line.strip_prefix("object ") {
                let mut fields = rest.splitn(3, ' ');
                let digest_hex = fields
                    .next()
                    .ok_or_else(|| refuse("object line without digest".to_string()))?;
                let byte_size = fields
                    .next()
                    .ok_or_else(|| refuse("object line without size".to_string()))?;
                let relative_path = fields
                    .next()
                    .ok_or_else(|| refuse("object line without path".to_string()))?;
                if !is_canonical_relative_path(relative_path) {
                    return Err(refuse(format!(
                        "object path {relative_path} is not a canonical relative path"
                    )));
                }
                objects.push(StateObjectEntryV1 {
                    relative_path: relative_path.to_string(),
                    byte_size: byte_size.parse::<u64>().map_err(|error| {
                        refuse(format!("object size {byte_size} is not a number: {error}"))
                    })?,
                    digest_hex: digest_hex.to_string(),
                });
            } else {
                return Err(refuse(format!("unrecognised manifest line: {line}")));
            }
        }
        let format_version =
            format_version.ok_or_else(|| refuse("missing format-version".to_string()))?;
        let root_format = root_format.ok_or_else(|| refuse("missing root-format".to_string()))?;
        let catalog_digest_hex =
            catalog_digest_hex.ok_or_else(|| refuse("missing catalog-digest".to_string()))?;
        let catalog_rows =
            catalog_rows.ok_or_else(|| refuse("missing catalog-rows".to_string()))?;
        let declared_digest =
            declared_digest.ok_or_else(|| refuse("missing root-digest".to_string()))?;
        let computed = hex_lower(&Sha256::digest(body.as_bytes()));
        if computed != declared_digest {
            return Err(typed(
                SearchPlaneErrorCodeV2::SearchTrackManifestDigestMismatch,
                format!(
                    "state-root manifest declares root-digest {declared_digest} but its body digests to {computed}"
                ),
            ));
        }
        let mut sorted = objects.clone();
        sorted.sort();
        sorted.dedup();
        if sorted != objects {
            return Err(refuse(
                "object lines are not in canonical sorted order".to_string(),
            ));
        }
        let mut sorted_directories = directories.clone();
        sorted_directories.sort();
        sorted_directories.dedup();
        if sorted_directories != directories {
            return Err(refuse(
                "dir lines are not in canonical sorted order".to_string(),
            ));
        }
        Ok(Self {
            format_version,
            root_format,
            catalog_digest_hex,
            catalog_rows,
            objects,
            directories,
        })
    }
}

/// A canonical manifest relative path: non-empty, forward-slash separated, no
/// `.`/`..` component, no absolute or drive prefix, no backslash.
#[must_use]
pub fn is_canonical_relative_path(path: &str) -> bool {
    if path.is_empty() || path.starts_with('/') || path.contains('\\') {
        return false;
    }
    path.split('/').all(|component| {
        !component.is_empty() && component != "." && component != ".." && component != " "
    })
}

/// Hex SHA-256 of a byte string; the one digest helper every caller shares.
#[must_use]
pub fn sha256_hex(bytes: &[u8]) -> String {
    hex_lower(&Sha256::digest(bytes))
}

/// Hex SHA-256 of a file's bytes, streamed so a large object is never read
/// into memory whole.
pub fn sha256_file_hex(path: &Path) -> Result<(String, u64), CoreError> {
    use std::io::Read as _;
    let mut file = fs::File::open(path).map_err(|error| storage("open object", path, &error))?;
    let mut hasher = Sha256::new();
    let mut buffer = vec![0_u8; 1 << 20];
    let mut total: u64 = 0;
    loop {
        let read = file
            .read(&mut buffer)
            .map_err(|error| storage("read object", path, &error))?;
        if read == 0 {
            break;
        }
        if let Some(chunk) = buffer.get(..read) {
            hasher.update(chunk);
        }
        total = total.saturating_add(u64::try_from(read).map_or(u64::MAX, |read| read));
    }
    Ok((hex_lower(&hasher.finalize()), total))
}

/// One lowercase hex digit. The nibble is masked, so the value is always
/// `0..=15` and neither branch can overflow.
fn hex_digit(nibble: u8) -> char {
    let nibble = nibble & 0x0f;
    if nibble < 10 {
        char::from(b'0'.wrapping_add(nibble))
    } else {
        char::from(b'a'.wrapping_add(nibble.wrapping_sub(10)))
    }
}

fn hex_lower(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len().saturating_mul(2));
    for byte in bytes {
        out.push(hex_digit(byte >> 4));
        out.push(hex_digit(byte & 0x0f));
    }
    out
}

/// `SQLite`'s own per-connection bookkeeping beside a database file.
///
/// These are vendor-native and their content is a function of the last
/// connection, not of the logical catalog — the manifest's `catalog-digest`
/// covers the logical content instead, so inventorying these bytes would
/// make a root's manifest depend on who opened it last.
#[must_use]
pub fn is_sqlite_sidecar_v1(name: &str) -> bool {
    [".sqlite-wal", ".sqlite-shm", ".sqlite-journal"]
        .iter()
        .any(|suffix| name.ends_with(suffix))
}

/// Enumerate every regular file under `root`, skipping `exclusions`
/// (top-level names) and the manifest/receipt files themselves.
///
/// A symlink, a special file, or an unreadable entry is refused typed: a
/// backup that silently skipped what it could not read is not a backup.
/// `SQLite` sidecars are the one documented exception — see
/// [`is_sqlite_sidecar_v1`].
pub fn inventory_state_root_v1(
    root: &Path,
    exclusions: &[&str],
) -> Result<Vec<StateObjectEntryV1>, CoreError> {
    let skip: BTreeSet<&str> = exclusions.iter().copied().collect();
    let mut objects = Vec::new();
    walk(root, root, &skip, &mut objects)?;
    objects.sort();
    Ok(objects)
}

fn walk(
    root: &Path,
    directory: &Path,
    skip: &BTreeSet<&str>,
    out: &mut Vec<StateObjectEntryV1>,
) -> Result<(), CoreError> {
    let entries =
        fs::read_dir(directory).map_err(|error| storage("read directory", directory, &error))?;
    for entry in entries {
        let entry = entry.map_err(|error| storage("read directory entry", directory, &error))?;
        let path = entry.path();
        let name = entry.file_name();
        let name = name.to_string_lossy().to_string();
        let relative = path
            .strip_prefix(root)
            .map_err(|error| storage("relativize object", &path, &error))?
            .to_string_lossy()
            .replace('\\', "/");
        if !relative.contains('/') && skip.contains(name.as_str()) {
            continue;
        }
        if is_sqlite_sidecar_v1(&name) {
            continue;
        }
        let metadata = fs::symlink_metadata(&path)
            .map_err(|error| storage("inspect object", &path, &error))?;
        if metadata.file_type().is_symlink() {
            return Err(typed(
                SearchPlaneErrorCodeV2::StateRootInsecure,
                format!(
                    "state root {} contains the symlink {relative}; an offline inventory refuses to follow it",
                    root.display()
                ),
            ));
        }
        if metadata.is_dir() {
            walk(root, &path, skip, out)?;
            continue;
        }
        if !metadata.is_file() {
            return Err(typed(
                SearchPlaneErrorCodeV2::StateRootInsecure,
                format!(
                    "state root {} contains the non-regular entry {relative}",
                    root.display()
                ),
            ));
        }
        let (digest_hex, byte_size) = sha256_file_hex(&path)?;
        out.push(StateObjectEntryV1 {
            relative_path: relative,
            byte_size,
            digest_hex,
        });
    }
    Ok(())
}

/// Enumerate every directory under `root`, skipping `exclusions`
/// (top-level names), as canonical relative paths in sorted order.
pub fn inventory_state_directories_v1(
    root: &Path,
    exclusions: &[&str],
) -> Result<Vec<String>, CoreError> {
    let skip: BTreeSet<&str> = exclusions.iter().copied().collect();
    let mut directories = Vec::new();
    walk_directories(root, root, &skip, &mut directories)?;
    directories.sort();
    Ok(directories)
}

fn walk_directories(
    root: &Path,
    directory: &Path,
    skip: &BTreeSet<&str>,
    out: &mut Vec<String>,
) -> Result<(), CoreError> {
    let entries =
        fs::read_dir(directory).map_err(|error| storage("read directory", directory, &error))?;
    for entry in entries {
        let entry = entry.map_err(|error| storage("read directory entry", directory, &error))?;
        let path = entry.path();
        let name = entry.file_name().to_string_lossy().to_string();
        let relative = path
            .strip_prefix(root)
            .map_err(|error| storage("relativize object", &path, &error))?
            .to_string_lossy()
            .replace('\\', "/");
        if !relative.contains('/') && skip.contains(name.as_str()) {
            continue;
        }
        let metadata = fs::symlink_metadata(&path)
            .map_err(|error| storage("inspect object", &path, &error))?;
        if metadata.file_type().is_symlink() {
            return Err(typed(
                SearchPlaneErrorCodeV2::StateRootInsecure,
                format!(
                    "state root {} contains the symlink {relative}; an offline inventory refuses to follow it",
                    root.display()
                ),
            ));
        }
        if !metadata.is_dir() {
            continue;
        }
        out.push(relative);
        walk_directories(root, &path, skip, out)?;
    }
    Ok(())
}

/// fsync a directory's own entries.
pub fn fsync_directory_v1(path: &Path) -> Result<(), CoreError> {
    let handle =
        fs::File::open(path).map_err(|error| storage("open directory to fsync", path, &error))?;
    handle
        .sync_all()
        .map_err(|error| storage("fsync directory", path, &error))
}

/// fsync one file's bytes.
pub fn fsync_file_v1(path: &Path) -> Result<(), CoreError> {
    let handle = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(path)
        .map_err(|error| storage("open file to fsync", path, &error))?;
    handle
        .sync_all()
        .map_err(|error| storage("fsync file", path, &error))
}

/// Which role a path plays in an offline operation.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum OfflineRootRoleV1 {
    /// An existing root that is only ever read.
    Source,
    /// A path the operation is about to create.
    Destination,
}

/// Broad system roots no offline state operation may target.
const BROAD_ROOT_DENYLIST: [&str; 22] = [
    "/",
    "/Applications",
    "/Library",
    "/Network",
    "/System",
    "/Users",
    "/Volumes",
    "/bin",
    "/boot",
    "/cores",
    "/dev",
    "/etc",
    "/home",
    "/lib",
    "/opt",
    "/private",
    "/proc",
    "/root",
    "/sbin",
    "/srv",
    "/sys",
    "/usr",
];

fn refuse(code: SearchPlaneErrorCodeV2, message: String) -> CoreError {
    typed(code, message)
}

/// Refuse a broad, unresolved, aliasing or symlinked target *before* any
/// mutation.
///
/// Returns the path unchanged on success; every caller must use the returned
/// path so no later step re-derives it.
pub fn refuse_broad_offline_target_v1(
    path: &Path,
    role: OfflineRootRoleV1,
) -> Result<PathBuf, CoreError> {
    if !path.is_absolute() {
        return Err(refuse(
            SearchPlaneErrorCodeV2::InvalidRequest,
            format!(
                "offline state {role:?} target {} must be an absolute path",
                path.display()
            ),
        ));
    }
    if path
        .components()
        .any(|component| matches!(component, Component::ParentDir))
    {
        return Err(refuse(
            SearchPlaneErrorCodeV2::InvalidRequest,
            format!(
                "offline state {role:?} target {} contains a `..` component; traversal is refused",
                path.display()
            ),
        ));
    }
    let components: Vec<Component<'_>> = path.components().collect();
    if components.len() < 3 || !matches!(components.first(), Some(Component::RootDir)) {
        return Err(refuse(
            SearchPlaneErrorCodeV2::InvalidRequest,
            format!(
                "offline state {role:?} target {} is too broad; it must name a directory at least two levels below the filesystem root",
                path.display()
            ),
        ));
    }
    if let Some(normalized) = lexical_normalize(path)
        && BROAD_ROOT_DENYLIST.contains(&normalized.as_str())
    {
        return Err(refuse(
            SearchPlaneErrorCodeV2::InvalidRequest,
            format!(
                "offline state {role:?} target {} is a broad system root; it is never a state root",
                path.display()
            ),
        ));
    }
    if let Ok(home) = std::env::var("HOME")
        && !home.is_empty()
    {
        let home = PathBuf::from(home);
        if path == home || home.starts_with(path) {
            return Err(refuse(
                SearchPlaneErrorCodeV2::InvalidRequest,
                format!(
                    "offline state {role:?} target {} is the home directory or one of its ancestors",
                    path.display()
                ),
            ));
        }
    }
    if let Ok(metadata) = fs::symlink_metadata(path)
        && metadata.file_type().is_symlink()
    {
        return Err(refuse(
            SearchPlaneErrorCodeV2::StateRootInsecure,
            format!(
                "offline state {role:?} target {} is a symlink; the state root must be a real directory",
                path.display()
            ),
        ));
    }
    Ok(path.to_path_buf())
}

fn lexical_normalize(path: &Path) -> Option<String> {
    let mut out: Vec<String> = Vec::new();
    for component in path.components() {
        match component {
            Component::RootDir => out.push(String::new()),
            Component::Normal(part) => out.push(part.to_string_lossy().to_string()),
            Component::CurDir => {}
            Component::ParentDir | Component::Prefix(_) => return None,
        }
    }
    Some(out.join("/"))
}

/// Refuse a source root that is not exactly this process's private root.
///
/// The daemon and the offline operations share one check: owned by the
/// effective uid, mode exactly `0700`, a real directory. A root others can
/// read or write is refused `STATE_ROOT_INSECURE` before any inventory.
#[cfg(unix)]
pub fn refuse_non_private_source_root_v1(root: &Path) -> Result<(), CoreError> {
    use std::os::unix::fs::MetadataExt as _;
    let metadata = match fs::symlink_metadata(root) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Err(refuse(
                SearchPlaneErrorCodeV2::NotFound,
                format!(
                    "offline state source root {} does not exist",
                    root.display()
                ),
            ));
        }
        Err(error) => return Err(storage("inspect source root", root, &error)),
    };
    if !metadata.is_dir() {
        return Err(refuse(
            SearchPlaneErrorCodeV2::StateRootFormatUnsupported,
            format!("source root {} is not a directory", root.display()),
        ));
    }
    let owner = rustix::process::geteuid().as_raw();
    let mode = metadata.mode() & 0o7777;
    if metadata.uid() != owner || mode != 0o700 {
        return Err(refuse(
            SearchPlaneErrorCodeV2::StateRootInsecure,
            format!(
                "source root {} is uid {} mode {mode:04o}; it must belong to uid {owner} and be exactly 0700",
                root.display(),
                metadata.uid()
            ),
        ));
    }
    Ok(())
}

#[cfg(not(unix))]
pub fn refuse_non_private_source_root_v1(root: &Path) -> Result<(), CoreError> {
    if !root.is_dir() {
        return Err(refuse(
            SearchPlaneErrorCodeV2::StateRootFormatUnsupported,
            format!("source root {} is not a directory", root.display()),
        ));
    }
    Ok(())
}

/// Refuse a destination that is not a fresh, empty, unaliased directory.
///
/// An existing **non-empty** destination is refused: an offline operation
/// that merged into a populated directory could leave a mixture of two
/// roots that no manifest describes.
pub fn refuse_non_empty_destination_v1(destination: &Path, source: &Path) -> Result<(), CoreError> {
    if destination == source {
        return Err(refuse(
            SearchPlaneErrorCodeV2::InvalidRequest,
            format!(
                "offline state operation refuses to write into its own source root {}",
                destination.display()
            ),
        ));
    }
    if let Ok(metadata) = fs::symlink_metadata(destination) {
        if !metadata.is_dir() {
            return Err(refuse(
                SearchPlaneErrorCodeV2::InvalidRequest,
                format!(
                    "offline state destination {} exists and is not a directory",
                    destination.display()
                ),
            ));
        }
        let entries = fs::read_dir(destination)
            .map_err(|error| storage("read destination", destination, &error))?;
        let mut count: u64 = 0;
        for entry in entries {
            let _entry = entry.map_err(|error| storage("read destination", destination, &error))?;
            count = count.saturating_add(1);
        }
        if count != 0 {
            return Err(refuse(
                SearchPlaneErrorCodeV2::InvalidRequest,
                format!(
                    "offline state destination {} is non-empty ({count} entries); a cutover target must be fresh",
                    destination.display()
                ),
            ));
        }
    }
    Ok(())
}

/// The staging directory an offline operation builds before its one rename.
#[must_use]
pub fn staging_directory_for_v1(destination: &Path) -> PathBuf {
    let name = destination.file_name().map_or_else(
        || "state-root".to_string(),
        |name| name.to_string_lossy().to_string(),
    );
    let parent = destination.parent().unwrap_or_else(|| Path::new("/"));
    parent.join(format!("{name}{STAGING_DIRECTORY_SUFFIX}"))
}

/// One atomic cutover: staging is complete and verified, so a single `rename`
/// publishes it and the parent directory is fsynced.
///
/// There is no intermediate state to observe: after a crash the destination
/// is either still absent (the old root stands) or the complete new root.
pub fn atomic_cutover_v1(staging: &Path, destination: &Path) -> Result<(), CoreError> {
    if !staging.is_dir() {
        return Err(typed(
            SearchPlaneErrorCodeV2::NotFound,
            format!("staging root {} is not a directory", staging.display()),
        ));
    }
    if destination.exists() {
        return Err(refuse(
            SearchPlaneErrorCodeV2::InvalidRequest,
            format!(
                "cutover destination {} already exists; a cutover never replaces a root",
                destination.display()
            ),
        ));
    }
    let parent = destination.parent().ok_or_else(|| {
        refuse(
            SearchPlaneErrorCodeV2::InvalidRequest,
            format!(
                "cutover destination {} has no parent",
                destination.display()
            ),
        )
    })?;
    fs::create_dir_all(parent).map_err(|error| storage("create cutover parent", parent, &error))?;
    fs::rename(staging, destination)
        .map_err(|error| storage("cutover rename staging -> destination", staging, &error))?;
    fsync_directory_v1(parent)?;
    Ok(())
}

/// The offline fault points a migration/backup/restore can be interrupted at.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StateMigrationFaultPointV1 {
    /// Every data object is durable, before the manifest exists.
    AfterDataSync,
    /// The manifest bytes are written, before they are fsynced.
    BeforeManifestSync,
    /// The manifest is durable, before the single cutover rename.
    BeforeCutoverRename,
    /// The rename happened, before its parent directory was fsynced.
    AfterCutoverRename,
}

/// A failpoint port, so a test can interrupt an offline operation at the
/// exact boundary the ordering rules are about.
///
/// The production implementation is [`NoStateMigrationFaultsV1`]; the
/// environment-driven subprocess hook lives in [`EnvironmentStateMigrationFaultV1`].
pub trait StateMigrationFaultPort: Send + Sync {
    fn reach(&self, point: StateMigrationFaultPointV1) -> Result<(), CoreError>;
}

/// The production fault port: nothing ever fires.
#[derive(Debug)]
pub struct NoStateMigrationFaultsV1;

impl StateMigrationFaultPort for NoStateMigrationFaultsV1 {
    fn reach(&self, _point: StateMigrationFaultPointV1) -> Result<(), CoreError> {
        Ok(())
    }
}

/// The environment-driven crash hook, mirroring the repo's crash-point
/// precedent.
///
/// A no-op unless `QUANTA_INDEX_STATE_MIGRATION_CRASH` names the boundary,
/// in which case the process exits without unwinding.
#[derive(Debug)]
pub struct EnvironmentStateMigrationFaultV1;

/// The environment variable naming the boundary to die at.
pub const STATE_MIGRATION_CRASH_ENV: &str = "QUANTA_INDEX_STATE_MIGRATION_CRASH";

/// The exit code a crashed offline operation leaves.
pub const STATE_MIGRATION_CRASH_EXIT_CODE: i32 = 88;

#[expect(
    clippy::exit,
    reason = "the subprocess-only crash matrix must terminate without unwinding, exactly as a crash would"
)]
fn exit_at_state_migration_boundary_v1(point: StateMigrationFaultPointV1) {
    if std::env::var(STATE_MIGRATION_CRASH_ENV)
        .is_ok_and(|configured| configured == fault_point_name_v1(point))
    {
        std::process::exit(STATE_MIGRATION_CRASH_EXIT_CODE);
    }
}

impl StateMigrationFaultPort for EnvironmentStateMigrationFaultV1 {
    fn reach(&self, point: StateMigrationFaultPointV1) -> Result<(), CoreError> {
        exit_at_state_migration_boundary_v1(point);
        Ok(())
    }
}

/// The stable name of a fault point, for the environment hook and receipts.
#[must_use]
pub const fn fault_point_name_v1(point: StateMigrationFaultPointV1) -> &'static str {
    match point {
        StateMigrationFaultPointV1::AfterDataSync => "after-data-sync",
        StateMigrationFaultPointV1::BeforeManifestSync => "before-manifest-sync",
        StateMigrationFaultPointV1::BeforeCutoverRename => "before-cutover-rename",
        StateMigrationFaultPointV1::AfterCutoverRename => "after-cutover-rename",
    }
}

/// Write the manifest last: bytes, fsync, then the parent directory.
///
/// Every caller has already made the data durable and reached
/// [`StateMigrationFaultPointV1::AfterDataSync`]; this is the only place a
/// manifest file is created, so there is exactly one ordering to audit.
pub fn write_root_manifest_last_v1(
    root: &Path,
    file_name: &str,
    manifest: &StateRootManifestV1,
    fault: &dyn StateMigrationFaultPort,
) -> Result<PathBuf, CoreError> {
    let path = root.join(file_name);
    if path.exists() {
        return Err(refuse(
            SearchPlaneErrorCodeV2::InvalidRequest,
            format!(
                "manifest {} already exists; a manifest is written exactly once per root",
                path.display()
            ),
        ));
    }
    fault.reach(StateMigrationFaultPointV1::BeforeManifestSync)?;
    fs::write(&path, manifest.encode())
        .map_err(|error| storage("write manifest", &path, &error))?;
    fsync_file_v1(&path)?;
    fsync_directory_v1(root)?;
    fault.reach(StateMigrationFaultPointV1::BeforeCutoverRename)?;
    Ok(path)
}

/// Read and strictly decode a root manifest.
pub fn read_root_manifest_v1(path: &Path) -> Result<StateRootManifestV1, CoreError> {
    if !path.is_file() {
        return Err(typed(
            SearchPlaneErrorCodeV2::NotFound,
            format!("state-root manifest {} is not a file", path.display()),
        ));
    }
    let bytes = fs::read_to_string(path).map_err(|error| storage("read manifest", path, &error))?;
    let manifest = StateRootManifestV1::decode(&bytes)?;
    if manifest.format_version != STATE_ROOT_MANIFEST_FORMAT_VERSION {
        return Err(typed(
            SearchPlaneErrorCodeV2::StateRootFormatUnsupported,
            format!(
                "state-root manifest {} declares format {}, this build writes and reads {STATE_ROOT_MANIFEST_FORMAT_VERSION}",
                path.display(),
                manifest.format_version
            ),
        ));
    }
    Ok(manifest)
}

/// Re-verify a root against a manifest: every advertised object must exist
/// with the exact size and digest, and no data object may have appeared that
/// the manifest does not name.
pub fn verify_root_against_manifest_v1(
    root: &Path,
    manifest_file_name: &str,
    manifest: &StateRootManifestV1,
    exclusions: &[&str],
) -> Result<(), CoreError> {
    let observed = inventory_state_root_v1(root, exclusions)?;
    let observed_directories = inventory_state_directories_v1(root, exclusions)?;
    let expected_directories: BTreeSet<&String> = manifest.directories.iter().collect();
    let observed_directory_set: BTreeSet<&String> = observed_directories.iter().collect();
    if let Some(missing) = expected_directories
        .difference(&observed_directory_set)
        .next()
    {
        return Err(typed(
            SearchPlaneErrorCodeV2::NotFound,
            format!(
                "root {} is missing the manifest directory {missing}",
                root.display()
            ),
        ));
    }
    if let Some(extra) = observed_directory_set
        .difference(&expected_directories)
        .next()
    {
        return Err(typed(
            SearchPlaneErrorCodeV2::SearchTrackManifestDigestMismatch,
            format!(
                "root {} holds the unadvertised directory {extra}; the manifest {} does not name it",
                root.display(),
                manifest_file_name
            ),
        ));
    }
    let expected: BTreeSet<&StateObjectEntryV1> = manifest.objects.iter().collect();
    let observed_set: BTreeSet<&StateObjectEntryV1> = observed.iter().collect();
    if let Some(missing) = expected.difference(&observed_set).next() {
        return Err(typed(
            SearchPlaneErrorCodeV2::NotFound,
            format!(
                "root {} is missing the manifest object {} ({} bytes, digest {})",
                root.display(),
                missing.relative_path,
                missing.byte_size,
                missing.digest_hex
            ),
        ));
    }
    if let Some(extra) = observed_set.difference(&expected).next() {
        return Err(typed(
            SearchPlaneErrorCodeV2::SearchTrackManifestDigestMismatch,
            format!(
                "root {} holds the unadvertised object {}; the manifest {} does not name it",
                root.display(),
                extra.relative_path,
                manifest_file_name
            ),
        ));
    }
    Ok(())
}
