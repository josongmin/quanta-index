//! Read-only XFS project quota observations through an already-held file descriptor.
//! The query requires CAP_SYS_ADMIN for project limits on Linux; callers must
//! keep the product process unprivileged and use a separately authorized reader.

use std::io;
use std::os::fd::AsFd;

#[cfg(target_os = "linux")]
mod ipc;
#[cfg(target_os = "linux")]
pub use ipc::{
    read_xfs_project_quota_via, read_xfs_project_quota_via_before, serve_xfs_project_quota,
    take_systemd_listener,
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct XfsProjectMembership {
    pub project_id: u32,
    pub inherits_project: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct XfsProjectQuota {
    pub project_id: u32,
    pub inherits_project: bool,
    pub accounting_enabled: bool,
    pub enforcement_enabled: bool,
    pub block_hard_limit_bytes: u64,
    pub inode_hard_limit: u64,
    pub used_bytes: u64,
    pub used_inodes: u64,
}

#[cfg(target_os = "linux")]
mod linux {
    use super::{XfsProjectMembership, XfsProjectQuota};
    use std::io;
    use std::os::fd::{AsFd, AsRawFd};

    #[repr(C)]
    #[derive(Default)]
    struct RawMembership {
        project_id: u32,
        inherits_project: u8,
        reserved: [u8; 3],
    }

    #[repr(C)]
    #[derive(Default)]
    struct RawQuota {
        project_id: u32,
        inherits_project: u8,
        accounting_enabled: u8,
        enforcement_enabled: u8,
        reserved: u8,
        block_hard_limit_bytes: u64,
        inode_hard_limit: u64,
        used_bytes: u64,
        used_inodes: u64,
    }

    unsafe extern "C" {
        fn quanta_read_xfs_project_membership(
            fd: libc::c_int,
            out: *mut RawMembership,
        ) -> libc::c_int;
        fn quanta_read_xfs_project_quota(fd: libc::c_int, out: *mut RawQuota) -> libc::c_int;
    }

    pub(super) fn membership(file: &impl AsFd) -> io::Result<XfsProjectMembership> {
        let mut raw = RawMembership::default();
        // SAFETY: the borrowed descriptor stays open for the call; C writes a
        // fixed-size repr(C) object and returns an errno value, never an FD.
        let error =
            unsafe { quanta_read_xfs_project_membership(file.as_fd().as_raw_fd(), &mut raw) };
        if error != 0 {
            return Err(io::Error::from_raw_os_error(error));
        }
        Ok(XfsProjectMembership {
            project_id: raw.project_id,
            inherits_project: raw.inherits_project == 1,
        })
    }

    pub(super) fn quota(root: &impl AsFd) -> io::Result<XfsProjectQuota> {
        let mut raw = RawQuota::default();
        // SAFETY: the borrowed descriptor stays open for the call; C writes a
        // fixed-size repr(C) object and returns an errno value.
        let error = unsafe { quanta_read_xfs_project_quota(root.as_fd().as_raw_fd(), &mut raw) };
        if error != 0 {
            return Err(io::Error::from_raw_os_error(error));
        }
        Ok(XfsProjectQuota {
            project_id: raw.project_id,
            inherits_project: raw.inherits_project == 1,
            accounting_enabled: raw.accounting_enabled == 1,
            enforcement_enabled: raw.enforcement_enabled == 1,
            block_hard_limit_bytes: raw.block_hard_limit_bytes,
            inode_hard_limit: raw.inode_hard_limit,
            used_bytes: raw.used_bytes,
            used_inodes: raw.used_inodes,
        })
    }
}

/// Read an XFS inode's project membership without opening a pathname.
pub fn read_xfs_project_membership(file: &impl AsFd) -> io::Result<XfsProjectMembership> {
    #[cfg(target_os = "linux")]
    {
        linux::membership(file)
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = file;
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "XFS quota is Linux-only",
        ))
    }
}

/// Read an installed, enforced XFS project quota from a held root directory.
/// Project quota reads require CAP_SYS_ADMIN in the calling Linux process.
pub fn read_xfs_project_quota(root: &impl AsFd) -> io::Result<XfsProjectQuota> {
    #[cfg(target_os = "linux")]
    {
        linux::quota(root)
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = root;
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "XFS quota is Linux-only",
        ))
    }
}

/// Require the caller to be a non-root writer without the capabilities that
/// can change quota state or override resource limits. This is a live check;
/// the launcher must also prevent later privilege gain.
pub fn require_unprivileged_xfs_writer() -> io::Result<()> {
    #[cfg(target_os = "linux")]
    {
        unsafe extern "C" {
            fn quanta_require_unprivileged_xfs_writer() -> libc::c_int;
        }
        // SAFETY: the C function only reads current Linux credentials.
        let error = unsafe { quanta_require_unprivileged_xfs_writer() };
        if error == 0 {
            Ok(())
        } else {
            Err(io::Error::from_raw_os_error(error))
        }
    }
    #[cfg(not(target_os = "linux"))]
    {
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "XFS quota is Linux-only",
        ))
    }
}

/// Confine Landlock-mediated filesystem writes by the calling Linux thread
/// and its future descendants.
///
/// `root` must be an already-authorized state root directory. If present,
/// `socket_parent` must be a separate dedicated directory: its rule permits
/// Unix socket creation and unlink only. This does not confine existing sibling
/// threads or writable descriptors inherited before the call. Landlock does
/// not mediate every metadata mutation, such as chmod. Success is irreversible
/// for this thread and requires Landlock ABI 3 or newer.
pub fn restrict_state_root_writes(
    root: &impl AsFd,
    socket_parent: Option<&dyn AsFd>,
) -> io::Result<()> {
    #[cfg(target_os = "linux")]
    {
        use std::os::fd::AsRawFd;

        unsafe extern "C" {
            fn quanta_restrict_state_root_writes(
                root_fd: libc::c_int,
                socket_fd: libc::c_int,
                has_socket_parent: libc::c_int,
            ) -> libc::c_int;
        }
        let (socket_fd, has_socket_parent) = match socket_parent {
            Some(directory) => (directory.as_fd().as_raw_fd(), 1),
            None => (-1, 0),
        };
        // SAFETY: the borrowed descriptors remain valid for the call. The C
        // adapter consumes neither descriptor and returns only an errno value.
        let error = unsafe {
            quanta_restrict_state_root_writes(
                root.as_fd().as_raw_fd(),
                socket_fd,
                has_socket_parent,
            )
        };
        if error == 0 {
            Ok(())
        } else {
            Err(io::Error::from_raw_os_error(error))
        }
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = (root, socket_parent);
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "Landlock write domains are Linux-only",
        ))
    }
}

/// Check descriptor numbers and refuse ambient writable regular files.
///
/// Call this during single-threaded startup. File descriptors 0-2 and those
/// marked `FD_CLOEXEC` are deliberate exceptions; the caller must separately
/// account for their write authority. This is not a check that all writable
/// regular files have been closed before applying Landlock. Pipes, sockets,
/// read-only files, and the scanner's own procfs directory are also permitted.
/// A descriptor at or above the ceiling returns `EMFILE`, an ambient writable
/// regular file returns `EBUSY`, and procfs errors retain their Linux errno.
pub fn require_stock_sqlite_descriptor_preflight(max_process_fds: u32) -> io::Result<()> {
    #[cfg(target_os = "linux")]
    {
        unsafe extern "C" {
            fn quanta_require_stock_sqlite_descriptor_preflight(
                max_process_fds: libc::c_uint,
            ) -> libc::c_int;
        }
        // SAFETY: the C adapter scans /proc/self/fd without retaining or
        // closing any caller-owned descriptor and returns an errno value.
        let error = unsafe { quanta_require_stock_sqlite_descriptor_preflight(max_process_fds) };
        if error == 0 {
            Ok(())
        } else {
            Err(io::Error::from_raw_os_error(error))
        }
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _limit = max_process_fds;
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "writable descriptor preflight is Linux-only",
        ))
    }
}

#[cfg(all(test, target_os = "linux"))]
mod write_domain_tests {
    use super::restrict_state_root_writes;
    use std::fs::File;
    use std::io;

    #[test]
    fn rejects_non_directory_and_duplicate_directory_descriptors() -> io::Result<()> {
        let root = File::open("/")?;
        let non_directory = File::open("/dev/null")?;
        assert_eq!(
            restrict_state_root_writes(&non_directory, None)
                .unwrap_err()
                .raw_os_error(),
            Some(libc::ENOTDIR)
        );
        assert_eq!(
            restrict_state_root_writes(&root, Some(&non_directory))
                .unwrap_err()
                .raw_os_error(),
            Some(libc::ENOTDIR)
        );
        assert_eq!(
            restrict_state_root_writes(&root, Some(&root))
                .unwrap_err()
                .raw_os_error(),
            Some(libc::EINVAL)
        );
        Ok(())
    }
}
