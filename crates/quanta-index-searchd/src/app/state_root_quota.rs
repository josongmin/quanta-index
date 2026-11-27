//! Kernel-enforced storage admission for one pre-provisioned StateRoot.

use std::path::{Component, Path, PathBuf};

use quanta_index_core::{CoreError, RequestBudgetV1};

use super::runtime::StateRootLease;

#[cfg(target_os = "linux")]
#[path = "state_root_quota_tree.rs"]
mod tree;

const MAX_STATE_ROOT_BYTES: u64 = 64 << 30;

/// Private SQLite temporary directory provisioned at the stock Native Root.
pub const STOCK_SQLITE_TEMP_DIR_NAME: &str = "sqlite-tmp";

/// Provisioning contract for an XFS project whose hard limits apply to the
/// entire mutable StateRoot. The verifier socket belongs to the privileged
/// installation, and the daemon queries it with the leased directory FD.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StateRootProjectQuotaV1 {
    verifier_socket: PathBuf,
    project_id: u32,
    hard_bytes: u64,
    hard_inodes: u64,
}

impl StateRootProjectQuotaV1 {
    pub fn new(
        verifier_socket: PathBuf,
        project_id: u32,
        hard_bytes: u64,
        hard_inodes: u64,
    ) -> Result<Self, CoreError> {
        if !verifier_socket.is_absolute()
            || verifier_socket.file_name().is_none()
            || verifier_socket
                .components()
                .any(|part| !matches!(part, Component::RootDir | Component::Normal(_)))
            || project_id == 0
            || hard_bytes == 0
            || hard_bytes > MAX_STATE_ROOT_BYTES
            || !hard_bytes.is_multiple_of(512)
            || hard_inodes == 0
        {
            return Err(CoreError::InvalidContract(
                "invalid StateRoot XFS quota verifier path, project ID or hard limits".into(),
            ));
        }
        Ok(Self {
            verifier_socket,
            project_id,
            hard_bytes,
            hard_inodes,
        })
    }

    /// Admit the actual root and its existing tree before opening the state
    /// adapters. Socket directories must be provisioned outside the data root.
    pub fn require(
        &self,
        lease: &StateRootLease,
        service_socket: &Path,
        max_tree_entries: u64,
        request: &RequestBudgetV1,
    ) -> Result<(), CoreError> {
        #[cfg(target_os = "linux")]
        {
            self.require_linux(lease, service_socket, max_tree_entries, request)
        }
        #[cfg(not(target_os = "linux"))]
        {
            let _inputs = (lease, service_socket, max_tree_entries, request);
            Err(CoreError::InvalidContract(format!(
                "StateRoot XFS quota profile for project {} ({} bytes, {} inodes via {}) requires Linux",
                self.project_id,
                self.hard_bytes,
                self.hard_inodes,
                self.verifier_socket.display(),
            )))
        }
    }

    #[cfg(target_os = "linux")]
    fn require_linux(
        &self,
        lease: &StateRootLease,
        service_socket: &Path,
        max_tree_entries: u64,
        request: &RequestBudgetV1,
    ) -> Result<(), CoreError> {
        use quanta_state_root_quota::{
            read_xfs_project_quota_via_before, require_unprivileged_xfs_writer,
        };

        request.checkpoint("state_root_xfs_quota")?;
        require_unprivileged_xfs_writer().map_err(|error| {
            CoreError::InvalidContract(format!("StateRoot XFS writer privileges: {error}"))
        })?;
        let root = lease.borrow_root_directory_read_v1()?;
        let root_path = lease.state_root_identity_v1();
        require_external_socket(root_path, &self.verifier_socket)?;
        require_external_socket(root_path, service_socket)?;
        let quota =
            read_xfs_project_quota_via_before(&self.verifier_socket, &root, request.deadline());
        request.checkpoint("state_root_xfs_quota")?;
        let quota = quota
            .map_err(|error| CoreError::Storage(format!("read StateRoot XFS quota: {error}")))?;
        if quota.project_id != self.project_id
            || !quota.inherits_project
            || !quota.accounting_enabled
            || !quota.enforcement_enabled
            || quota.block_hard_limit_bytes != self.hard_bytes
            || quota.inode_hard_limit != self.hard_inodes
            || quota.used_bytes > self.hard_bytes
            || quota.used_inodes > self.hard_inodes
        {
            return Err(CoreError::InvalidContract(format!(
                "StateRoot XFS quota differs from the configured project {} and hard limits ({} bytes, {} inodes): {quota:?}",
                self.project_id, self.hard_bytes, self.hard_inodes
            )));
        }
        let scan =
            tree::require_xfs_project_tree(&root, self.project_id, max_tree_entries, &mut || {
                request
                    .checkpoint("state_root_xfs_quota")
                    .map_err(std::io::Error::other)
            });
        request.checkpoint("state_root_xfs_quota")?;
        scan.map_err(|error| {
            CoreError::Storage(format!("verify StateRoot XFS project tree: {error}"))
        })?;
        root.require_current_v1()
    }
}

#[cfg(target_os = "linux")]
fn require_external_socket(root: &Path, socket: &Path) -> Result<(), CoreError> {
    let parent = socket.parent().ok_or_else(|| {
        CoreError::InvalidContract("StateRoot XFS profile requires a socket directory".into())
    })?;
    let parent = parent.canonicalize().map_err(|error| {
        CoreError::Storage(format!("open StateRoot profile socket directory: {error}"))
    })?;
    if parent.starts_with(root) {
        return Err(CoreError::InvalidContract(
            "StateRoot XFS profile requires sockets outside its managed root".into(),
        ));
    }
    Ok(())
}

/// Check the dedicated process before stock SQLite writer threads start.
/// The launcher installs the OS limit. The signed configuration sets its ceiling.
pub fn require_stock_sqlite_launch(max_process_fds: u32) -> Result<(), CoreError> {
    #[cfg(target_os = "linux")]
    {
        use nix::sys::resource::{RLIM_INFINITY, Resource, getrlimit, rlim_t};

        let (soft, hard) = getrlimit(Resource::RLIMIT_NOFILE).map_err(|error| {
            CoreError::Storage(format!("read stock SQLite RLIMIT_NOFILE: {error}"))
        })?;
        let maximum = rlim_t::from(max_process_fds);
        if soft == 0 || hard == RLIM_INFINITY || soft > maximum || hard > maximum {
            return Err(CoreError::InvalidContract(format!(
                "stock SQLite RLIMIT_NOFILE soft {soft} and hard {hard} must be positive and at or below {maximum}",
            )));
        }
        quanta_state_root_quota::require_stock_sqlite_descriptor_preflight(max_process_fds).map_err(
            |error| {
                CoreError::InvalidContract(format!("stock SQLite descriptor preflight: {error}"))
            },
        )
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _limit = max_process_fds;
        Err(CoreError::InvalidContract(
            "stock SQLite write confinement requires Linux".into(),
        ))
    }
}

/// Enter the stock SQLite write domain on the current thread. Callers
/// must hold the admitted quota lease and start writers from this thread.
pub fn restrict_stock_sqlite_writes(
    lease: &StateRootLease,
    service_socket: Option<&Path>,
) -> Result<(), CoreError> {
    #[cfg(target_os = "linux")]
    {
        use quanta_state_root_quota::read_xfs_project_membership;
        use rustix::fs::{Mode, OFlags, openat2};

        let root = lease.borrow_root_directory_read_v1()?;
        let root_path = lease.state_root_identity_v1();
        let temp = std::env::var_os("SQLITE_TMPDIR")
            .map(PathBuf::from)
            .ok_or_else(|| {
                CoreError::InvalidContract(
                    "stock SQLite requires SQLITE_TMPDIR inside the quota root".into(),
                )
            })?;
        if temp.as_os_str() != root_path.join(STOCK_SQLITE_TEMP_DIR_NAME).as_os_str() {
            return Err(CoreError::InvalidContract(
                "stock SQLite SQLITE_TMPDIR must name a private directory inside the quota root"
                    .into(),
            ));
        }
        let temp_directory = std::fs::File::from(
            openat2(
                &root,
                STOCK_SQLITE_TEMP_DIR_NAME,
                OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
                Mode::empty(),
                tree::RESOLVE_TREE,
            )
            .map_err(|error| {
                CoreError::Storage(format!("open stock SQLite temp directory: {error}"))
            })?,
        );
        require_private_write_domain_directory(&temp_directory, &temp)?;
        let membership = read_xfs_project_membership(&root).map_err(|error| {
            CoreError::Storage(format!("read held StateRoot XFS project: {error}"))
        })?;
        if membership.project_id == 0 || !membership.inherits_project {
            return Err(CoreError::InvalidContract(
                "held StateRoot no longer belongs to an inherited XFS project".into(),
            ));
        }
        tree::require_member(&temp_directory, membership.project_id, true).map_err(|error| {
            CoreError::Storage(format!(
                "verify stock SQLite temp directory XFS project: {error}"
            ))
        })?;
        let socket_parent = service_socket
            .map(|socket| open_stock_socket_directory(root_path, socket))
            .transpose()?;
        quanta_state_root_quota::restrict_state_root_writes(
            &root,
            socket_parent
                .as_ref()
                .map(|file| file as &dyn std::os::fd::AsFd),
        )
        .map_err(|error| CoreError::Storage(format!("restrict stock SQLite writes: {error}")))?;
        root.require_current_v1()
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _inputs = (lease, service_socket);
        Err(CoreError::InvalidContract(
            "stock SQLite write confinement requires Linux".into(),
        ))
    }
}

#[cfg(target_os = "linux")]
fn open_private_write_domain_directory(path: &Path) -> Result<std::fs::File, CoreError> {
    use rustix::fs::{Mode, OFlags, open};

    let io = |error| {
        CoreError::Storage(format!(
            "stock Catalog directory {}: {error}",
            path.display()
        ))
    };
    if path.canonicalize().map_err(io)?.as_os_str() != path.as_os_str() {
        return Err(CoreError::InvalidContract(
            "stock Catalog directories must use canonical paths".into(),
        ));
    }
    let file = std::fs::File::from(
        open(
            path,
            OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
            Mode::empty(),
        )
        .map_err(|error| io(std::io::Error::from(error)))?,
    );
    require_private_write_domain_directory(&file, path)?;
    Ok(file)
}

#[cfg(target_os = "linux")]
fn require_private_write_domain_directory(
    file: &std::fs::File,
    path: &Path,
) -> Result<(), CoreError> {
    use std::os::unix::fs::MetadataExt as _;

    let metadata = file.metadata().map_err(|error| {
        CoreError::Storage(format!(
            "stock Catalog directory {}: {error}",
            path.display()
        ))
    })?;
    if metadata.uid() != rustix::process::geteuid().as_raw() || metadata.mode() & 0o777 != 0o700 {
        return Err(CoreError::InvalidContract(
            "stock Catalog directories must be owned and private".into(),
        ));
    }
    Ok(())
}

#[cfg(target_os = "linux")]
fn open_stock_socket_directory(root: &Path, socket: &Path) -> Result<std::fs::File, CoreError> {
    use rustix::fs::{AtFlags, Dir, FileType, statat};
    use std::os::unix::ffi::OsStrExt as _;

    require_external_socket(root, socket)?;
    let (Some(parent), Some(name)) = (socket.parent(), socket.file_name()) else {
        return Err(CoreError::InvalidContract(
            "stock Catalog requires a service socket path".into(),
        ));
    };
    let directory = open_private_write_domain_directory(parent)?;
    let io = |error| CoreError::Storage(format!("inspect stock Catalog socket directory: {error}"));
    for entry in Dir::read_from(&directory).map_err(io)? {
        let entry = entry.map_err(io)?;
        let leaf = entry.file_name();
        if matches!(leaf.to_bytes(), b"." | b"..") {
            continue;
        }
        if leaf.to_bytes() == name.as_bytes() {
            let observed = statat(&directory, leaf, AtFlags::SYMLINK_NOFOLLOW).map_err(io)?;
            if FileType::from_raw_mode(observed.st_mode) == FileType::Socket
                && observed.st_uid == rustix::process::geteuid().as_raw()
            {
                continue;
            }
        }
        return Err(CoreError::InvalidContract(
            "stock Catalog requires a dedicated directory for its service socket".into(),
        ));
    }
    Ok(directory)
}

#[cfg(test)]
mod tests {
    use super::{MAX_STATE_ROOT_BYTES, StateRootProjectQuotaV1};

    #[cfg(target_os = "linux")]
    #[test]
    fn stock_catalog_socket_exception_accepts_only_its_socket() -> anyhow::Result<()> {
        use std::{
            fs,
            os::unix::{fs::DirBuilderExt as _, net::UnixListener},
        };

        let base = tempfile::tempdir_in(std::env::temp_dir().canonicalize()?)?;
        let root = base.path().join("state");
        let endpoint = base.path().join("listener");
        fs::DirBuilder::new().mode(0o700).create(&root)?;
        fs::DirBuilder::new().mode(0o700).create(&endpoint)?;
        let socket = endpoint.join("native.sock");
        let _empty = super::open_stock_socket_directory(&root, &socket)?;
        let _listener = UnixListener::bind(&socket)?;
        let _existing = super::open_stock_socket_directory(&root, &socket)?;
        let foreign = endpoint.join("unrelated");
        fs::write(&foreign, b"must not be granted unlink authority")?;
        assert!(super::open_stock_socket_directory(&root, &socket).is_err());
        assert!(super::open_stock_socket_directory(&root, &root.join("native.sock")).is_err());
        assert_eq!(
            fs::read(foreign)?.as_slice(),
            b"must not be granted unlink authority"
        );
        Ok(())
    }

    #[test]
    fn state_root_xfs_quota_requires_explicit_aligned_hard_limits() {
        let valid_socket = "/run/quanta/quota.sock";
        assert!(
            StateRootProjectQuotaV1::new(valid_socket.into(), 17, MAX_STATE_ROOT_BYTES, 400)
                .is_ok()
        );
        for (socket, project, bytes, inodes) in [
            ("quota.sock", 17, 4096, 400),
            ("/run/../quota.sock", 17, 4096, 400),
            (valid_socket, 0, 4096, 400),
            (valid_socket, 17, 0, 400),
            (valid_socket, 17, 511, 400),
            (valid_socket, 17, 4096, 0),
            (valid_socket, 17, u64::MAX, 400),
        ] {
            assert!(StateRootProjectQuotaV1::new(socket.into(), project, bytes, inodes).is_err());
        }
    }
}
