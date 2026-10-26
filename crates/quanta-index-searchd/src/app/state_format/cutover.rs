//! Held directory identity for an offline state-root cutover.

use std::ffi::{OsStr, OsString};
use std::fs::{self, File};
use std::os::unix::fs::MetadataExt as _;
use std::path::{Component, Path, PathBuf};

use quanta_index_contract::SearchPlaneErrorCodeV2;
use quanta_index_core::{CoreError, RequestBudgetV1};
use rustix::fs::{
    AtFlags, Mode, OFlags, RenameFlags, fstat, mkdirat, open, openat, renameat_with, statat,
};

use super::{
    OfflineRootRoleV1, refuse, refuse_broad_offline_target_v1, staging_directory_for_v1, storage,
    typed,
};

#[derive(Clone, Copy, Eq, PartialEq)]
struct Identity {
    device: u64,
    inode: u64,
}

fn identity(stat: &rustix::fs::Stat, path: &Path) -> Result<Identity, CoreError> {
    Ok(Identity {
        // dev_t is signed on macOS and already u64 on Linux. Widen first so
        // the checked conversion preserves negative-device refusal on both.
        device: u64::try_from(i128::from(stat.st_dev))
            .map_err(|error| storage("decode cutover device", path, &error))?,
        inode: stat.st_ino,
    })
}

fn entry(parent: &File, name: &OsStr, path: &Path) -> Result<Option<Identity>, CoreError> {
    match statat(parent, Path::new(name), AtFlags::SYMLINK_NOFOLLOW) {
        Ok(stat) => identity(&stat, path).map(Some),
        Err(rustix::io::Errno::NOENT) => Ok(None),
        Err(error) => Err(storage("inspect cutover entry", path, &error)),
    }
}

fn require_absent(parent: &File, name: &OsStr, path: &Path) -> Result<(), CoreError> {
    if entry(parent, name, path)?.is_some() {
        return Err(refuse(
            SearchPlaneErrorCodeV2::InvalidRequest,
            format!("cutover target {} already exists", path.display()),
        ));
    }
    Ok(())
}

fn open_parent(path: &Path) -> Result<(File, Identity), CoreError> {
    // Stable aliases such as /var -> /private/var are part of the public
    // offline path contract. Walk their resolved target without following any
    // further links, then recheck the caller's path against the pinned inode.
    let canonical =
        fs::canonicalize(path).map_err(|error| storage("resolve cutover parent", path, &error))?;
    let flags = OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC;
    let mut held = File::from(
        open("/", flags, Mode::empty())
            .map_err(|error| storage("open cutover filesystem root", path, &error))?,
    );
    for component in canonical.components() {
        match component {
            Component::RootDir => {}
            Component::Normal(name) => {
                held = File::from(
                    openat(&held, Path::new(name), flags, Mode::empty()).map_err(|error| {
                        storage("open cutover parent without links", path, &error)
                    })?,
                );
            }
            Component::Prefix(_) | Component::CurDir | Component::ParentDir => {
                return Err(refuse(
                    SearchPlaneErrorCodeV2::InvalidRequest,
                    format!(
                        "cutover parent {} is not a canonical absolute path",
                        path.display()
                    ),
                ));
            }
        }
    }
    let pinned = identity(
        &fstat(&held).map_err(|error| storage("stat held cutover parent", path, &error))?,
        path,
    )?;
    require_parent_identity(path, pinned)?;
    Ok((held, pinned))
}

fn require_parent_identity(path: &Path, expected: Identity) -> Result<(), CoreError> {
    let current =
        fs::metadata(path).map_err(|error| storage("recheck cutover parent", path, &error))?;
    if current.dev() != expected.device || current.ino() != expected.inode {
        return Err(refuse(
            SearchPlaneErrorCodeV2::StateRootInsecure,
            format!(
                "cutover parent {} changed after it was pinned",
                path.display()
            ),
        ));
    }
    Ok(())
}

fn open_source(parent: &File, name: &OsStr, path: &Path) -> Result<(File, Identity), CoreError> {
    let flags = OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC;
    let source = File::from(
        openat(parent, Path::new(name), flags, Mode::empty()).map_err(|error| {
            if error == rustix::io::Errno::NOENT {
                typed(
                    SearchPlaneErrorCodeV2::NotFound,
                    format!("staging root {} is not a directory", path.display()),
                )
            } else {
                storage("open staging root without links", path, &error)
            }
        })?,
    );
    let pinned = identity(
        &fstat(&source).map_err(|error| storage("stat held staging root", path, &error))?,
        path,
    )?;
    if entry(parent, name, path)? != Some(pinned) {
        return Err(refuse(
            SearchPlaneErrorCodeV2::StateRootInsecure,
            format!(
                "staging root {} changed after it was pinned",
                path.display()
            ),
        ));
    }
    Ok((source, pinned))
}

/// A staging root pinned from creation until its one namespace publication.
///
/// This is a namespace capability, not a byte-capacity reservation. On any
/// preparation error after `mkdirat` succeeds, the staging entry is retained
/// for operator inspection; it is never removed through a path-only guess.
pub struct PreparedStateRootV1 {
    parent_path: PathBuf,
    parent: File,
    parent_identity: Identity,
    staging_path: PathBuf,
    staging_name: OsString,
    staging: File,
    staging_identity: Identity,
    destination_path: PathBuf,
    destination_name: OsString,
}

impl PreparedStateRootV1 {
    /// The original, exclusively created staging path to populate.
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.staging_path
    }

    fn require_original_fd(&self) -> Result<(), CoreError> {
        require_parent_identity(&self.parent_path, self.parent_identity)?;
        let pinned = identity(
            &fstat(&self.staging)
                .map_err(|error| storage("stat original staging FD", &self.staging_path, &error))?,
            &self.staging_path,
        )?;
        if pinned != self.staging_identity {
            return Err(refuse(
                SearchPlaneErrorCodeV2::StateRootInsecure,
                format!(
                    "staging FD {} is not the original directory",
                    self.staging_path.display()
                ),
            ));
        }
        Ok(())
    }

    /// Publish the original directory without replacing any target entry.
    /// A post-rename proof or sync failure reports failure while leaving the
    /// already-published namespace intact. Repeating this call with the same
    /// held capability can resync only that exact original inode.
    pub fn publish_noreplace(&self) -> Result<(), CoreError> {
        self.require_original_fd()?;
        let source = entry(&self.parent, &self.staging_name, &self.staging_path)?;
        let target = entry(&self.parent, &self.destination_name, &self.destination_path)?;
        if source.is_none() && target == Some(self.staging_identity) {
            self.parent.sync_all().map_err(|error| {
                storage("resync held cutover parent", &self.parent_path, &error)
            })?;
            return require_parent_identity(&self.parent_path, self.parent_identity);
        }
        if source != Some(self.staging_identity) {
            return Err(refuse(
                SearchPlaneErrorCodeV2::StateRootInsecure,
                format!(
                    "staging root {} no longer names its original directory",
                    self.staging_path.display()
                ),
            ));
        }
        if target.is_some() {
            return Err(refuse(
                SearchPlaneErrorCodeV2::InvalidRequest,
                format!(
                    "cutover destination {} already exists",
                    self.destination_path.display()
                ),
            ));
        }
        require_absent(&self.parent, &self.destination_name, &self.destination_path)?;
        require_parent_identity(&self.parent_path, self.parent_identity)?;
        renameat_with(
            &self.parent,
            Path::new(&self.staging_name),
            &self.parent,
            Path::new(&self.destination_name),
            RenameFlags::NOREPLACE,
        )
        .map_err(|error| {
            if error == rustix::io::Errno::EXIST {
                refuse(
                    SearchPlaneErrorCodeV2::InvalidRequest,
                    format!(
                        "cutover destination {} already exists",
                        self.destination_path.display()
                    ),
                )
            } else {
                storage(
                    "cutover rename staging -> destination",
                    &self.staging_path,
                    &error,
                )
            }
        })?;
        if entry(&self.parent, &self.destination_name, &self.destination_path)?
            != Some(self.staging_identity)
            || entry(&self.parent, &self.staging_name, &self.staging_path)?.is_some()
        {
            return Err(refuse(
                SearchPlaneErrorCodeV2::StateRootInsecure,
                format!(
                    "published cutover {} does not match original staging inode",
                    self.destination_path.display()
                ),
            ));
        }
        self.parent
            .sync_all()
            .map_err(|error| storage("fsync held cutover parent", &self.parent_path, &error))?;
        require_parent_identity(&self.parent_path, self.parent_identity)
    }
}

fn names(staging: &Path, destination: &Path) -> Result<(PathBuf, OsString, OsString), CoreError> {
    let parent = destination.parent().ok_or_else(|| {
        refuse(
            SearchPlaneErrorCodeV2::InvalidRequest,
            format!(
                "cutover destination {} has no parent",
                destination.display()
            ),
        )
    })?;
    if staging.parent() != Some(parent) {
        return Err(refuse(
            SearchPlaneErrorCodeV2::InvalidRequest,
            format!(
                "cutover staging {} and destination {} must share one parent",
                staging.display(),
                destination.display()
            ),
        ));
    }
    let source = staging.file_name().ok_or_else(|| {
        refuse(
            SearchPlaneErrorCodeV2::InvalidRequest,
            format!("cutover staging {} has no filename", staging.display()),
        )
    })?;
    let target = destination.file_name().ok_or_else(|| {
        refuse(
            SearchPlaneErrorCodeV2::InvalidRequest,
            format!(
                "cutover destination {} has no filename",
                destination.display()
            ),
        )
    })?;
    if !staging.is_absolute() || !destination.is_absolute() || source == target {
        return Err(refuse(
            SearchPlaneErrorCodeV2::InvalidRequest,
            "cutover requires distinct absolute staging and destination names".into(),
        ));
    }
    Ok((
        parent.to_path_buf(),
        source.to_os_string(),
        target.to_os_string(),
    ))
}

fn pin_existing(staging: &Path, destination: &Path) -> Result<PreparedStateRootV1, CoreError> {
    let (parent_path, staging_name, destination_name) = names(staging, destination)?;
    let (parent, parent_identity) = open_parent(&parent_path)?;
    let (staging_fd, staging_identity) = open_source(&parent, &staging_name, staging)?;
    Ok(PreparedStateRootV1 {
        parent_path,
        parent,
        parent_identity,
        staging_path: staging.to_path_buf(),
        staging_name,
        staging: staging_fd,
        staging_identity,
        destination_path: destination.to_path_buf(),
        destination_name,
    })
}

/// Entry-scope compatibility cutover. This pins the staging inode only when
/// called; callers requiring creation-to-publish identity use
/// [`prepare_staging_root_v1`] and its returned capability instead.
pub(super) fn atomic_cutover_v1(staging: &Path, destination: &Path) -> Result<(), CoreError> {
    pin_existing(staging, destination)?.publish_noreplace()
}

/// Create and pin an empty private staging root before any object is copied.
///
/// The destination parent must already exist. This method never creates an
/// uncontrolled ancestor, and never removes an uncertain staging entry on
/// error. A post-`mkdirat` error leaves that exclusive entry for inspection.
pub fn prepare_staging_root_v1(
    destination: &Path,
    budget: &RequestBudgetV1,
) -> Result<PreparedStateRootV1, CoreError> {
    let _validated = refuse_broad_offline_target_v1(destination, OfflineRootRoleV1::Destination)?;
    let staging_path = staging_directory_for_v1(destination);
    let (parent_path, staging_name, destination_name) = names(&staging_path, destination)?;
    let (parent, parent_identity) = open_parent(&parent_path)?;
    require_absent(&parent, &destination_name, destination)?;
    require_absent(&parent, &staging_name, &staging_path)?;
    budget.checkpoint("state-root:before-staging-create")?;
    require_parent_identity(&parent_path, parent_identity)?;
    mkdirat(
        &parent,
        Path::new(&staging_name),
        Mode::from_raw_mode(0o700),
    )
    .map_err(|error| {
        if error == rustix::io::Errno::EXIST {
            refuse(
                SearchPlaneErrorCodeV2::InvalidRequest,
                format!("staging root {} already exists", staging_path.display()),
            )
        } else {
            storage("create private staging root", &staging_path, &error)
        }
    })?;
    // Do not recheck the budget after birth: a cancellation cannot discard
    // the namespace witness. Any failure below leaves the new entry in place.
    let (staging, staging_identity) = open_source(&parent, &staging_name, &staging_path)?;
    parent
        .sync_all()
        .map_err(|error| storage("fsync new staging entry", &parent_path, &error))?;
    require_parent_identity(&parent_path, parent_identity)?;
    Ok(PreparedStateRootV1 {
        parent_path,
        parent,
        parent_identity,
        staging_path,
        staging_name,
        staging,
        staging_identity,
        destination_path: destination.to_path_buf(),
        destination_name,
    })
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::os::unix::fs::symlink;
    use std::time::{Duration, Instant};

    use anyhow::{Context as _, Result, ensure};
    use quanta_index_core::RequestBudgetV1;

    use super::prepare_staging_root_v1;

    fn target() -> Result<(tempfile::TempDir, std::path::PathBuf)> {
        let temp = tempfile::tempdir()?;
        let parent = temp.path().join("held-parent");
        fs::create_dir(&parent)?;
        Ok((temp, parent.join("published")))
    }

    #[test]
    fn prepared_cutover_keeps_original_inode_and_refuses_existing_or_symlink_target() -> Result<()>
    {
        let (_temp, destination) = target()?;
        let prepared = prepare_staging_root_v1(&destination, &RequestBudgetV1::unbounded())?;
        let staging = prepared.path().to_path_buf();
        fs::write(staging.join("fixed-object"), b"abc")?;

        fs::create_dir(&destination)?;
        let occupied = prepared.publish_noreplace();
        ensure!(occupied.is_err(), "an existing target must refuse cutover");
        ensure!(destination.is_dir(), "existing target was replaced");
        ensure!(
            staging.join("fixed-object").is_file(),
            "original staging disappeared"
        );
        fs::remove_dir(&destination)?;

        let victim = destination.with_file_name("victim");
        fs::write(&victim, b"victim")?;
        symlink(&victim, &destination)?;
        let linked = prepared.publish_noreplace();
        ensure!(linked.is_err(), "a symlink target must refuse cutover");
        ensure!(
            fs::read_link(&destination)? == victim,
            "target symlink was replaced"
        );
        ensure!(
            fs::read(&victim)? == b"victim",
            "symlink victim was modified"
        );
        fs::remove_file(&destination)?;

        prepared.publish_noreplace()?;
        prepared.publish_noreplace()?;
        ensure!(
            fs::read(destination.join("fixed-object"))? == b"abc",
            "published bytes differ"
        );
        ensure!(!staging.exists(), "original staging name still exists");
        Ok(())
    }

    #[test]
    fn prepared_cutover_refuses_replaced_staging_and_parent() -> Result<()> {
        let (_temp, destination) = target()?;
        let prepared = prepare_staging_root_v1(&destination, &RequestBudgetV1::unbounded())?;
        let staging = prepared.path().to_path_buf();
        let displaced = staging.with_file_name("displaced-staging");
        fs::rename(&staging, &displaced)?;
        fs::create_dir(&staging)?;
        ensure!(
            prepared.publish_noreplace().is_err(),
            "replaced staging was accepted"
        );
        ensure!(!destination.exists(), "replaced staging was published");
        ensure!(displaced.is_dir(), "original pinned directory was lost");

        let (_other_temp, other_destination) = target()?;
        let other_prepared =
            prepare_staging_root_v1(&other_destination, &RequestBudgetV1::unbounded())?;
        let parent = other_destination.parent().context("destination parent")?;
        let displaced_parent = parent.with_file_name("displaced-parent");
        fs::rename(parent, &displaced_parent)?;
        fs::create_dir(parent)?;
        ensure!(
            other_prepared.publish_noreplace().is_err(),
            "replaced parent was accepted"
        );
        ensure!(
            !other_destination.exists(),
            "replaced parent received a cutover"
        );
        ensure!(displaced_parent.is_dir(), "original pinned parent was lost");
        Ok(())
    }

    #[test]
    fn expired_budget_refuses_before_staging_birth() -> Result<()> {
        let (_temp, destination) = target()?;
        let deadline = Instant::now()
            .checked_sub(Duration::from_secs(1))
            .context("expired fixture deadline is not representable")?;
        let expired = RequestBudgetV1::until(deadline);
        let refused = prepare_staging_root_v1(&destination, &expired);
        ensure!(refused.is_err(), "expired budget created a staging root");
        ensure!(!destination.exists(), "destination was created");
        let staging = super::staging_directory_for_v1(&destination);
        ensure!(!staging.exists(), "staging was created after budget expiry");
        Ok(())
    }
}
