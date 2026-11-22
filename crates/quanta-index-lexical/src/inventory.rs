//! Engine-independent boot inventory of persisted lexical generations.

#![expect(
    clippy::redundant_pub_crate,
    reason = "the module is private to the crate; `pub(crate)` is the visibility its items need across the crate's modules, and the workspace's `unreachable_pub = deny` forbids the bare `pub`"
)]

use crate::boot_metadata::{
    quarantined_by_scrub_at, read_lexical_sealed_identity_at, sealed_identity_entry_present_at,
};

use quanta_index_contract::{GenerationSnapshot, SearchPlaneTrackKind};
use quanta_index_core::domains::generation::{
    GenerationQuarantineReasonV1, GenerationStorageKeyV1, InventoriedSealedGenerationV1,
    QuarantinedGenerationV1, SealedGenerationInventoryV1,
};
use quanta_index_core::{CoreError, RECLAIM_AREA_DIR_NAME, SealedGenerationScanPort};

use std::{
    ffi::{OsStr, OsString},
    fs::{File, Metadata},
    os::unix::{ffi::OsStringExt as _, fs::MetadataExt as _},
    path::{Path, PathBuf},
};

use rustix::fs::{AtFlags, Dir, FileType, Mode, OFlags, Stat, fstat, open, openat, statat};

const DIRECTORY_FLAGS: OFlags = OFlags::RDONLY
    .union(OFlags::CLOEXEC)
    .union(OFlags::DIRECTORY)
    .union(OFlags::NOFOLLOW)
    .union(OFlags::NONBLOCK);

fn io_error(error: rustix::io::Errno) -> std::io::Error {
    std::io::Error::from_raw_os_error(error.raw_os_error())
}

fn storage(action: &str, path: &Path, error: impl std::fmt::Display) -> CoreError {
    CoreError::Storage(format!("lexical: {action} {}: {error}", path.display()))
}

#[derive(Clone, Copy, Eq, PartialEq)]
struct DirectoryIdentity {
    device: u64,
    inode: u64,
    bytes: u64,
    links: u64,
    owner: u32,
    group: u32,
    mode: u32,
    modified: (i64, i64),
    changed: (i64, i64),
}

impl DirectoryIdentity {
    fn from(metadata: &Metadata) -> Self {
        Self {
            device: metadata.dev(),
            inode: metadata.ino(),
            bytes: metadata.len(),
            links: metadata.nlink(),
            owner: metadata.uid(),
            group: metadata.gid(),
            mode: metadata.mode(),
            modified: (metadata.mtime(), metadata.mtime_nsec()),
            changed: (metadata.ctime(), metadata.ctime_nsec()),
        }
    }
}

fn next_entry(
    entries: &mut Dir,
    directory: &File,
    directory_path: &Path,
) -> Result<Option<(OsString, Stat)>, CoreError> {
    while let Some(entry) = entries.read() {
        let entry =
            entry.map_err(|error| storage("read directory entry below", directory_path, error))?;
        let bytes = entry.file_name().to_bytes();
        if bytes == b"." || bytes == b".." {
            continue;
        }
        let name = OsString::from_vec(bytes.to_vec());
        let metadata = statat(directory, Path::new(&name), AtFlags::SYMLINK_NOFOLLOW)
            .map_err(|error| storage("inspect", &directory_path.join(&name), error))?;
        return Ok(Some((name, metadata)));
    }
    Ok(None)
}

fn open_child_directory(
    parent: &File,
    parent_path: &Path,
    name: &OsStr,
    observed: &Stat,
) -> Result<(File, DirectoryIdentity), CoreError> {
    let path = parent_path.join(name);
    let directory = openat(parent, Path::new(name), DIRECTORY_FLAGS, Mode::empty())
        .map(File::from)
        .map_err(|error| storage("open directory", &path, error))?;
    let current =
        fstat(&directory).map_err(|error| storage("inspect opened directory", &path, error))?;
    if current.st_dev != observed.st_dev || current.st_ino != observed.st_ino {
        return Err(CoreError::Storage(format!(
            "lexical: directory changed while inventorying {}",
            path.display()
        )));
    }
    let identity = DirectoryIdentity::from(
        &directory
            .metadata()
            .map_err(|error| storage("inspect opened directory", &path, error))?,
    );
    Ok((directory, identity))
}

fn require_named_directory_current(
    parent: &File,
    parent_path: &Path,
    name: &OsStr,
    opened: &File,
    before: DirectoryIdentity,
) -> Result<(), CoreError> {
    let path = parent_path.join(name);
    let named = statat(parent, Path::new(name), AtFlags::SYMLINK_NOFOLLOW)
        .map_err(|error| storage("reinspect directory", &path, error))?;
    let current =
        fstat(opened).map_err(|error| storage("reinspect opened directory", &path, error))?;
    let after = DirectoryIdentity::from(
        &opened
            .metadata()
            .map_err(|error| storage("reinspect opened directory", &path, error))?,
    );
    if FileType::from_raw_mode(named.st_mode) != FileType::Directory
        || named.st_dev != current.st_dev
        || named.st_ino != current.st_ino
        || after != before
    {
        return Err(CoreError::Storage(format!(
            "lexical: directory changed while inventorying {}",
            path.display()
        )));
    }
    Ok(())
}

fn require_path_directory_current(
    path: &Path,
    opened: &File,
    before: DirectoryIdentity,
) -> Result<(), CoreError> {
    let named = std::fs::symlink_metadata(path)
        .map_err(|error| storage("reinspect inventory root path", path, error))?;
    let after = opened
        .metadata()
        .map_err(|error| storage("reinspect opened inventory root", path, error))?;
    if named.file_type().is_symlink()
        || !named.is_dir()
        || DirectoryIdentity::from(&named) != before
        || DirectoryIdentity::from(&after) != before
    {
        return Err(CoreError::Storage(format!(
            "lexical: inventory root path changed while inventorying {}",
            path.display()
        )));
    }
    Ok(())
}

fn require_path_names_opened_directory(path: &Path, opened: &File) -> Result<(), CoreError> {
    let named = std::fs::symlink_metadata(path)
        .map_err(|error| storage("reinspect inventory directory path", path, error))?;
    let current = opened
        .metadata()
        .map_err(|error| storage("reinspect opened inventory directory", path, error))?;
    if named.file_type().is_symlink()
        || !named.is_dir()
        || (named.dev(), named.ino()) != (current.dev(), current.ino())
    {
        return Err(CoreError::Storage(format!(
            "lexical: inventory directory path changed while inventorying {}",
            path.display()
        )));
    }
    Ok(())
}

fn open_inventory_root(
    lexical_root: &Path,
) -> Result<Option<(File, File, OsString, DirectoryIdentity)>, CoreError> {
    let Some(name) = lexical_root.file_name() else {
        return Err(CoreError::Storage(format!(
            "lexical: inventory root {} has no final directory name",
            lexical_root.display()
        )));
    };
    let parent_path = lexical_root
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    let parent = match open(parent_path, DIRECTORY_FLAGS, Mode::empty()) {
        Ok(directory) => File::from(directory),
        Err(error) if error == rustix::io::Errno::NOENT => {
            return match crate::sealed_path_io::optional_entry_metadata(lexical_root) {
                Ok(None) => Ok(None),
                Ok(Some(_)) => Err(CoreError::Storage(format!(
                    "lexical: inventory root {} exists below a missing parent",
                    lexical_root.display()
                ))),
                Err(inspect_error) => Err(storage(
                    "inspect inventory root",
                    lexical_root,
                    inspect_error,
                )),
            };
        }
        Err(error) => {
            return Err(storage(
                "open inventory parent",
                parent_path,
                io_error(error),
            ));
        }
    };
    let directory = match openat(&parent, Path::new(name), DIRECTORY_FLAGS, Mode::empty()) {
        Ok(directory) => File::from(directory),
        Err(error) if error == rustix::io::Errno::NOENT => {
            return match crate::sealed_path_io::optional_entry_metadata(lexical_root) {
                Ok(None) => Ok(None),
                Ok(Some(_)) => Err(CoreError::Storage(format!(
                    "lexical: inventory root {} exists but cannot be opened without following links",
                    lexical_root.display()
                ))),
                Err(inspect_error) => Err(storage(
                    "inspect inventory root",
                    lexical_root,
                    inspect_error,
                )),
            };
        }
        Err(error) => {
            return Err(storage(
                "open inventory root",
                lexical_root,
                io_error(error),
            ));
        }
    };
    let identity = DirectoryIdentity::from(
        &directory
            .metadata()
            .map_err(|error| storage("inspect opened inventory root", lexical_root, error))?,
    );
    Ok(Some((parent, directory, name.to_os_string(), identity)))
}

/// Engine-independent lexical boot scanner.
///
/// The persisted identity and quarantine receipt are DATA contracts, so boot
/// inventory remains available when the optional Legacy serving engine is not
/// compiled.
#[derive(Clone, Debug)]
pub struct LexicalGenerationScanner {
    lexical_root: PathBuf,
}

impl LexicalGenerationScanner {
    #[must_use]
    pub fn new(lexical_root: PathBuf) -> Self {
        Self { lexical_root }
    }
}

impl SealedGenerationScanPort for LexicalGenerationScanner {
    fn inventory_sealed_generations(&self) -> Result<SealedGenerationInventoryV1, CoreError> {
        inventory_sealed_generations(&self.lexical_root)
    }
}

#[derive(Debug, Eq, PartialEq)]
pub(crate) enum InventoryGenerationError {
    Quarantined(QuarantinedGenerationV1),
    Infrastructure(CoreError),
}

fn unreadable_identity(generation_dir: &Path, error: CoreError) -> InventoryGenerationError {
    if matches!(error, CoreError::Storage(_)) {
        InventoryGenerationError::Infrastructure(error)
    } else {
        InventoryGenerationError::Quarantined(quarantine(
            generation_dir.to_path_buf(),
            GenerationQuarantineReasonV1::IdentityUnreadable,
            error.to_string(),
        ))
    }
}

/// Inventory the sealed generations under a lexical state root (QI-BB-026).
///
/// Reads each generation's sealed identity and optional quarantine receipt,
/// and nothing else: no content sidecar is hashed and no index is opened. A
/// directory that is not a canonical family or `g<N>`, an invalid identity
/// and an identity that does not own its directory are quarantined with a
/// reason. Generations without a sealed identity are skipped unless a durable
/// quarantine receipt still fences them. I/O failures propagate rather than
/// masquerading as corruption.
pub fn inventory_sealed_generations(
    lexical_root: &Path,
) -> Result<SealedGenerationInventoryV1, CoreError> {
    let mut inventory = SealedGenerationInventoryV1::default();
    let Some((lexical_parent, lexical_directory, lexical_name, lexical_identity)) =
        open_inventory_root(lexical_root)?
    else {
        return Ok(inventory);
    };
    let mut family_entries =
        Dir::read_from(&lexical_directory).map_err(|error| storage("list", lexical_root, error))?;
    while let Some((family_name, family_metadata)) =
        next_entry(&mut family_entries, &lexical_directory, lexical_root)?
    {
        if FileType::from_raw_mode(family_metadata.st_mode) != FileType::Directory {
            continue;
        }
        // Reclaims in progress, finished by the reclaim port (QI-BB-003).
        if family_name.as_os_str() == OsStr::new(RECLAIM_AREA_DIR_NAME) {
            continue;
        }
        if family_name.as_os_str() == OsStr::new("lexical-object-pool") {
            continue;
        }
        if !family_name
            .to_str()
            .is_some_and(GenerationStorageKeyV1::is_canonical_name)
        {
            inventory.quarantined.push(quarantine(
                lexical_root.join(&family_name),
                GenerationQuarantineReasonV1::NonCanonicalLayout,
                "directory is not a canonical generation family; it needs explicit migration"
                    .to_string(),
            ));
            continue;
        }
        let family_path = lexical_root.join(&family_name);
        let (family_directory, family_identity) = open_child_directory(
            &lexical_directory,
            lexical_root,
            family_name.as_os_str(),
            &family_metadata,
        )?;
        let mut generation_entries = Dir::read_from(&family_directory)
            .map_err(|error| storage("list", &family_path, error))?;
        while let Some((generation_name, generation_metadata)) =
            next_entry(&mut generation_entries, &family_directory, &family_path)?
        {
            if FileType::from_raw_mode(generation_metadata.st_mode) != FileType::Directory {
                continue;
            }
            let generation_dir = family_path.join(&generation_name);
            let (generation_directory, generation_identity) = open_child_directory(
                &family_directory,
                &family_path,
                generation_name.as_os_str(),
                &generation_metadata,
            )?;
            let inspected =
                inventory_generation_dir(lexical_root, &generation_dir, &generation_directory);
            // A scrub receipt is independent of the sealed identity. Once it
            // exists, typed identity damage must not erase the durable fence.
            // Non-canonical directory names are not valid receipt owners.
            let carries_receipt = generation_name
                .to_str()
                .and_then(GenerationStorageKeyV1::generation_of_dir_name)
                .is_some();
            let receipt = if carries_receipt {
                Some(quarantined_by_scrub_at(
                    &generation_directory,
                    &generation_dir,
                ))
            } else {
                None
            };
            let rechecked =
                inventory_generation_dir(lexical_root, &generation_dir, &generation_directory);
            let rechecked_receipt = if carries_receipt {
                Some(quarantined_by_scrub_at(
                    &generation_directory,
                    &generation_dir,
                ))
            } else {
                None
            };
            if rechecked != inspected || rechecked_receipt != receipt {
                return Err(CoreError::Storage(format!(
                    "lexical: generation metadata changed while inventorying {}",
                    generation_dir.display()
                )));
            }
            require_named_directory_current(
                &family_directory,
                &family_path,
                generation_name.as_os_str(),
                &generation_directory,
                generation_identity,
            )?;
            // Infrastructure failure remains a boot failure even if a receipt
            // is present: the scanner cannot prove which bytes were readable.
            let inspected = match inspected {
                Err(InventoryGenerationError::Infrastructure(error)) => return Err(error),
                inspected => inspected,
            };
            if let Some(receipt) = receipt {
                match receipt {
                    // A generation the scrub proved corrupt is quarantined by
                    // its receipt, not served (QI-BB-017, QI-BB-026).
                    Ok(Some(quarantined)) => {
                        inventory.quarantined.push(quarantined);
                        continue;
                    }
                    Ok(None) => {}
                    Err(CoreError::Typed {
                        code:
                            quanta_index_contract::SearchPlaneErrorCodeV2::GenerationScrubReceiptInvalid,
                        message,
                    }) => {
                        inventory.quarantined.push(quarantine(
                            generation_dir,
                            GenerationQuarantineReasonV1::IdentityUnreadable,
                            message,
                        ));
                        continue;
                    }
                    Err(error) => return Err(error),
                }
            }
            match inspected {
                Ok(Some(identity)) => inventory.sealed.push(InventoriedSealedGenerationV1 {
                    identity,
                    path: generation_dir,
                }),
                Ok(None) => {}
                Err(InventoryGenerationError::Quarantined(quarantined)) => {
                    inventory.quarantined.push(quarantined);
                }
                Err(InventoryGenerationError::Infrastructure(_)) => unreachable!(
                    "infrastructure inventory failures return before receipt classification"
                ),
            }
        }
        require_named_directory_current(
            &lexical_directory,
            lexical_root,
            family_name.as_os_str(),
            &family_directory,
            family_identity,
        )?;
    }
    let lexical_parent_path = lexical_root
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    require_named_directory_current(
        &lexical_parent,
        lexical_parent_path,
        &lexical_name,
        &lexical_directory,
        lexical_identity,
    )?;
    require_path_names_opened_directory(lexical_parent_path, &lexical_parent)?;
    require_path_directory_current(lexical_root, &lexical_directory, lexical_identity)?;
    Ok(inventory)
}

/// Inventory one generation directory.
///
/// `Ok(Some)` is a sealed identity that owns the directory; `Ok(None)` is an
/// in-progress build. `Err` distinguishes quarantine from infrastructure
/// failure.
pub(crate) fn inventory_generation_dir(
    lexical_root: &Path,
    generation_dir: &Path,
    generation_directory: &File,
) -> Result<Option<GenerationSnapshot>, InventoryGenerationError> {
    let Some(generation_name) = generation_dir.file_name().and_then(|name| name.to_str()) else {
        return Err(InventoryGenerationError::Quarantined(quarantine(
            generation_dir.to_path_buf(),
            GenerationQuarantineReasonV1::NonCanonicalLayout,
            "generation directory name is not UTF-8".to_string(),
        )));
    };
    if GenerationStorageKeyV1::generation_of_dir_name(generation_name).is_none() {
        return Err(InventoryGenerationError::Quarantined(quarantine(
            generation_dir.to_path_buf(),
            GenerationQuarantineReasonV1::NonCanonicalLayout,
            "generation directory is not `g<N>`; it needs explicit migration".to_string(),
        )));
    }
    let identity_present = sealed_identity_entry_present_at(generation_directory, generation_dir)
        .map_err(|error| unreadable_identity(generation_dir, error))?;
    if !identity_present {
        return Ok(None);
    }
    let identity = read_lexical_sealed_identity_at(generation_directory, generation_dir)
        .map_err(|error| unreadable_identity(generation_dir, error))?;
    if identity.track != SearchPlaneTrackKind::Lexical
        || GenerationStorageKeyV1::for_repo_revision(&identity.repo_id, &identity.revision_id)
            .generation_dir(lexical_root, identity.manifest_generation)
            != generation_dir
    {
        return Err(InventoryGenerationError::Quarantined(quarantine(
            generation_dir.to_path_buf(),
            GenerationQuarantineReasonV1::ScopeMismatch,
            format!(
                "sealed identity names {:?} repo={} revision={} generation={}, which does not own this directory",
                identity.track,
                identity.repo_id.as_str(),
                identity.revision_id.as_str(),
                identity.manifest_generation.get()
            ),
        )));
    }
    Ok(Some(identity))
}

pub(crate) fn quarantine(
    path: PathBuf,
    reason: GenerationQuarantineReasonV1,
    detail: String,
) -> QuarantinedGenerationV1 {
    QuarantinedGenerationV1 {
        track: SearchPlaneTrackKind::Lexical,
        path,
        reason,
        detail,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn named_directory_mutation_or_replacement_is_refused_after_open()
    -> Result<(), Box<dyn std::error::Error>> {
        let temp = tempfile::tempdir()?;
        let parent_path = temp.path().join("parent");
        let path = parent_path.join("generation");
        let displaced = parent_path.join("displaced");
        std::fs::create_dir_all(&path)?;
        let parent = File::from(open(&parent_path, DIRECTORY_FLAGS, Mode::empty())?);
        let observed = statat(&parent, Path::new("generation"), AtFlags::SYMLINK_NOFOLLOW)?;
        let (opened, identity) =
            open_child_directory(&parent, &parent_path, OsStr::new("generation"), &observed)?;
        std::fs::write(path.join("late-entry"), b"late")?;
        if !matches!(
            require_named_directory_current(
                &parent,
                &parent_path,
                OsStr::new("generation"),
                &opened,
                identity,
            ),
            Err(CoreError::Storage(_))
        ) {
            return Err("in-place directory mutation was not refused".into());
        }
        std::fs::rename(&path, &displaced)?;
        std::fs::create_dir(&path)?;
        if !matches!(
            require_named_directory_current(
                &parent,
                &parent_path,
                OsStr::new("generation"),
                &opened,
                identity,
            ),
            Err(CoreError::Storage(_))
        ) {
            return Err("named directory replacement was not refused".into());
        }

        let named_parent = temp.path().join("named-parent");
        let displaced_parent = temp.path().join("displaced-parent");
        std::fs::create_dir(&named_parent)?;
        let opened_parent = File::from(open(&named_parent, DIRECTORY_FLAGS, Mode::empty())?);
        std::fs::rename(&named_parent, &displaced_parent)?;
        std::os::unix::fs::symlink(&displaced_parent, &named_parent)?;
        if !matches!(
            require_path_names_opened_directory(&named_parent, &opened_parent),
            Err(CoreError::Storage(_))
        ) {
            return Err("directory path replacement by a symlink was not refused".into());
        }
        Ok(())
    }
}
