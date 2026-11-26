//! Linux sealed-FD process backend.

mod custody;
#[cfg(target_arch = "x86_64")]
mod solver_seccomp_v1;
mod spawn;

use crate::{ExplicitFdProcessErrorV1, ExplicitFdSpawnRequestV1, SpawnedProcessV1};
use std::mem::MaybeUninit;
use std::os::fd::{AsFd, AsRawFd, BorrowedFd, FromRawFd, OwnedFd};
use std::os::unix::process::ExitStatusExt;
use std::process::ExitStatus;

pub(crate) use custody::{PinnedExecutablePlatformV1, PinnedReadOnlyFilePlatformV1};

#[derive(Debug)]
pub(crate) struct ProcessLifetimeGuardV1;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct DescriptorIdentityV1 {
    pub(crate) device_v1: u64,
    pub(crate) inode_v1: u64,
    pub(crate) size_v1: u64,
    pub(crate) mode_v1: u32,
    pub(crate) modified_seconds_v1: i64,
    pub(crate) modified_nanoseconds_v1: i64,
    pub(crate) changed_seconds_v1: i64,
    pub(crate) changed_nanoseconds_v1: i64,
    pub(crate) regular_file_v1: bool,
    pub(crate) directory_v1: bool,
}

pub(crate) fn descriptor_is_cloexec_v1(
    descriptor_v1: BorrowedFd<'_>,
) -> Result<bool, ExplicitFdProcessErrorV1> {
    // SAFETY: F_GETFD reads flags from a live borrowed descriptor.
    let flags_v1 = unsafe { libc::fcntl(descriptor_v1.as_raw_fd(), libc::F_GETFD) };
    if flags_v1 < 0 {
        return Err(descriptor_error_v1("fcntl_getfd_v1"));
    }
    Ok(flags_v1 & libc::FD_CLOEXEC != 0)
}

pub(crate) fn set_descriptor_cloexec_v1(
    descriptor_v1: BorrowedFd<'_>,
) -> Result<(), ExplicitFdProcessErrorV1> {
    // SAFETY: F_GETFD reads flags from a live borrowed descriptor.
    let flags_v1 = unsafe { libc::fcntl(descriptor_v1.as_raw_fd(), libc::F_GETFD) };
    if flags_v1 < 0 {
        return Err(descriptor_error_v1("fcntl_getfd_before_set_cloexec_v1"));
    }
    // SAFETY: F_SETFD updates only descriptor flags and never assumes ownership.
    let result_v1 = unsafe {
        libc::fcntl(
            descriptor_v1.as_raw_fd(),
            libc::F_SETFD,
            flags_v1 | libc::FD_CLOEXEC,
        )
    };
    if result_v1 < 0 {
        return Err(descriptor_error_v1("fcntl_setfd_cloexec_v1"));
    }
    Ok(())
}

pub(crate) fn descriptor_is_read_only_v1(
    descriptor_v1: BorrowedFd<'_>,
) -> Result<bool, ExplicitFdProcessErrorV1> {
    // SAFETY: F_GETFL reads flags from a live borrowed descriptor.
    let flags_v1 = unsafe { libc::fcntl(descriptor_v1.as_raw_fd(), libc::F_GETFL) };
    if flags_v1 < 0 {
        return Err(descriptor_error_v1("fcntl_getfl_v1"));
    }
    Ok(flags_v1 & libc::O_ACCMODE == libc::O_RDONLY)
}

pub(crate) fn duplicate_cloexec_v1(
    descriptor_v1: BorrowedFd<'_>,
) -> Result<OwnedFd, ExplicitFdProcessErrorV1> {
    duplicate_cloexec_at_least_v1(descriptor_v1, 3)
}

pub(super) fn duplicate_cloexec_at_least_v1(
    descriptor_v1: BorrowedFd<'_>,
    minimum_v1: i32,
) -> Result<OwnedFd, ExplicitFdProcessErrorV1> {
    // SAFETY: F_DUPFD_CLOEXEC atomically creates a new owned descriptor.
    let duplicate_v1 =
        unsafe { libc::fcntl(descriptor_v1.as_raw_fd(), libc::F_DUPFD_CLOEXEC, minimum_v1) };
    if duplicate_v1 < 0 {
        return Err(descriptor_error_v1("fcntl_dupfd_cloexec_v1"));
    }
    // SAFETY: fcntl returned a fresh descriptor owned by this function.
    Ok(unsafe { OwnedFd::from_raw_fd(duplicate_v1) })
}

pub(crate) fn descriptor_identity_v1(
    descriptor_v1: BorrowedFd<'_>,
) -> Result<DescriptorIdentityV1, ExplicitFdProcessErrorV1> {
    let mut stat_v1 = MaybeUninit::<libc::stat>::uninit();
    // SAFETY: fstat initializes the pointed-to stat on success.
    if unsafe { libc::fstat(descriptor_v1.as_raw_fd(), stat_v1.as_mut_ptr()) } != 0 {
        return Err(descriptor_error_v1("fstat_descriptor_v1"));
    }
    // SAFETY: successful fstat initialized the value.
    let stat_v1 = unsafe { stat_v1.assume_init() };
    Ok(DescriptorIdentityV1 {
        device_v1: stat_v1.st_dev,
        inode_v1: stat_v1.st_ino,
        size_v1: u64::try_from(stat_v1.st_size)
            .map_err(|_| ExplicitFdProcessErrorV1::InvalidExecutableDescriptor)?,
        mode_v1: stat_v1.st_mode,
        modified_seconds_v1: stat_v1.st_mtime,
        modified_nanoseconds_v1: stat_v1.st_mtime_nsec,
        changed_seconds_v1: stat_v1.st_ctime,
        changed_nanoseconds_v1: stat_v1.st_ctime_nsec,
        regular_file_v1: stat_v1.st_mode & libc::S_IFMT == libc::S_IFREG,
        directory_v1: stat_v1.st_mode & libc::S_IFMT == libc::S_IFDIR,
    })
}

pub(crate) fn descriptor_is_pipe_v1(
    descriptor_v1: BorrowedFd<'_>,
) -> Result<bool, ExplicitFdProcessErrorV1> {
    Ok(descriptor_identity_v1(descriptor_v1)?.mode_v1 & libc::S_IFMT == libc::S_IFIFO)
}

pub(crate) fn duplicate_inherited_read_pipe_v1(
    raw_fd_v1: i32,
) -> Result<OwnedFd, ExplicitFdProcessErrorV1> {
    // SAFETY: F_GETFD observes the numeric child-entry descriptor without
    // assuming Rust ownership.
    let flags_v1 = unsafe { libc::fcntl(raw_fd_v1, libc::F_GETFD) };
    if flags_v1 < 0 || flags_v1 & libc::FD_CLOEXEC != 0 {
        return Err(ExplicitFdProcessErrorV1::InvalidInheritedDescriptor);
    }
    // SAFETY: F_DUPFD_CLOEXEC atomically creates an owned duplicate.
    let duplicate_v1 = unsafe { libc::fcntl(raw_fd_v1, libc::F_DUPFD_CLOEXEC, 3) };
    if duplicate_v1 < 0 {
        return Err(ExplicitFdProcessErrorV1::InvalidInheritedDescriptor);
    }
    // SAFETY: fcntl returned a fresh descriptor owned here.
    let duplicate_v1 = unsafe { OwnedFd::from_raw_fd(duplicate_v1) };
    if !descriptor_is_read_only_v1(duplicate_v1.as_fd())?
        || !descriptor_is_pipe_v1(duplicate_v1.as_fd())?
    {
        return Err(ExplicitFdProcessErrorV1::InvalidInheritedDescriptor);
    }
    Ok(duplicate_v1)
}

pub(crate) fn spawn_v1(
    request_v1: &ExplicitFdSpawnRequestV1<'_>,
) -> Result<SpawnedProcessV1, ExplicitFdProcessErrorV1> {
    spawn::spawn_v1(request_v1)
}

pub(crate) fn spawn_borrowed_argv_v1(
    executable_v1: &crate::PinnedExecutableV1,
    arguments_v1: &[&std::ffi::CStr],
    environment_v1: crate::BorrowedEnvironmentV1<'_>,
    working_directory_v1: Option<&crate::PinnedWorkingDirectoryV1>,
    source_fds_v1: Option<[BorrowedFd<'_>; 2]>,
    ceiling_v1: libc::rlim_t,
    confined_z3_v1: bool,
) -> Result<SpawnedProcessV1, ExplicitFdProcessErrorV1> {
    spawn::spawn_borrowed_argv_v1(
        executable_v1,
        arguments_v1,
        environment_v1,
        working_directory_v1,
        source_fds_v1,
        ceiling_v1,
        confined_z3_v1,
    )
}

pub(crate) fn wait_pid_v1(pid_v1: libc::pid_t) -> Result<ExitStatus, ExplicitFdProcessErrorV1> {
    let mut status_v1 = 0;
    loop {
        // SAFETY: pid is launcher-owned and status points to initialized storage.
        let result_v1 = unsafe { libc::waitpid(pid_v1, &mut status_v1, 0) };
        if result_v1 == pid_v1 {
            return Ok(ExitStatus::from_raw(status_v1));
        }
        if result_v1 < 0 && errno_v1() == libc::EINTR {
            continue;
        }
        return Err(spawn_error_v1("waitpid_v1"));
    }
}

pub(crate) fn try_wait_pid_v1(
    pid_v1: libc::pid_t,
) -> Result<Option<ExitStatus>, ExplicitFdProcessErrorV1> {
    let mut status_v1 = 0;
    loop {
        // SAFETY: pid is launcher-owned; WNOHANG never blocks the caller.
        let result_v1 = unsafe { libc::waitpid(pid_v1, &mut status_v1, libc::WNOHANG) };
        if result_v1 == pid_v1 {
            return Ok(Some(ExitStatus::from_raw(status_v1)));
        }
        if result_v1 == 0 {
            return Ok(None);
        }
        if errno_v1() != libc::EINTR {
            return Err(spawn_error_v1("waitpid_nohang_v1"));
        }
    }
}

pub(crate) fn observe_pid_exit_without_reap_v1(
    pid_v1: libc::pid_t,
    nohang_v1: bool,
) -> Result<bool, ExplicitFdProcessErrorV1> {
    let mut info_v1 = MaybeUninit::<libc::siginfo_t>::zeroed();
    let options_v1 = libc::WEXITED | libc::WNOWAIT | if nohang_v1 { libc::WNOHANG } else { 0 };
    loop {
        // SAFETY: waitid initializes siginfo on success while WNOWAIT keeps the
        // launcher-owned leader waitable, preserving its process-group ID.
        let result_v1 = unsafe {
            libc::waitid(
                libc::P_PID,
                pid_v1 as libc::id_t,
                info_v1.as_mut_ptr(),
                options_v1,
            )
        };
        if result_v1 == 0 {
            // SAFETY: successful waitid initialized the zeroed siginfo value.
            let info_v1 = unsafe { info_v1.assume_init() };
            // SAFETY: si_pid reads the process field of initialized siginfo.
            return Ok(unsafe { info_v1.si_pid() } == pid_v1);
        }
        if errno_v1() != libc::EINTR {
            return Err(spawn_error_v1("waitid_wnowait_v1"));
        }
    }
}

pub(crate) fn kill_process_v1(
    pid_v1: libc::pid_t,
    isolated_process_group_v1: bool,
) -> Result<(), ExplicitFdProcessErrorV1> {
    if isolated_process_group_v1 {
        let group_v1 = pid_v1
            .checked_neg()
            .ok_or_else(|| spawn_error_v1("kill_process_group_range_v1"))?;
        // SAFETY: a negative target addresses the child-owned isolated group.
        if unsafe { libc::kill(group_v1, libc::SIGKILL) } != 0 && errno_v1() != libc::ESRCH {
            return Err(spawn_error_v1("kill_process_group_v1"));
        }
        // Also signal the owned leader PID. This closes custody if the direct
        // child moved itself out of the group after exec; ESRCH means it exited
        // between the group signal and this fallback.
        // SAFETY: kill does not dereference memory and pid is launcher-owned.
        if unsafe { libc::kill(pid_v1, libc::SIGKILL) } != 0 && errno_v1() != libc::ESRCH {
            return Err(spawn_error_v1("kill_process_leader_v1"));
        }
        return Ok(());
    }
    // SAFETY: kill does not dereference memory and pid is launcher-owned.
    if unsafe { libc::kill(pid_v1, libc::SIGKILL) } != 0 {
        return Err(spawn_error_v1("kill_v1"));
    }
    Ok(())
}

pub(crate) fn kill_process_group_after_leader_exit_v1(
    pid_v1: libc::pid_t,
) -> Result<(), ExplicitFdProcessErrorV1> {
    let group_v1 = pid_v1
        .checked_neg()
        .ok_or_else(|| spawn_error_v1("kill_process_group_range_v1"))?;
    // SAFETY: waitid(WNOWAIT) already proved that the launcher-owned group
    // leader exited while preserving the group identity until the final reap.
    if unsafe { libc::kill(group_v1, libc::SIGKILL) } != 0 && errno_v1() != libc::ESRCH {
        return Err(spawn_error_v1("kill_process_group_after_leader_exit_v1"));
    }
    Ok(())
}

pub(crate) fn terminate_and_reap_v1(pid_v1: libc::pid_t, isolated_process_group_v1: bool) {
    let mut status_v1 = 0;
    if isolated_process_group_v1 {
        // The unreaped leader keeps this process-group identity from being
        // reused. Signal the full group before reaping that leader.
        let _ = kill_process_v1(pid_v1, true);
        spawn::reap_failed_child_v1(pid_v1);
        return;
    }
    loop {
        // SAFETY: nonblocking ownership check for the launcher-owned child.
        let result_v1 = unsafe { libc::waitpid(pid_v1, &mut status_v1, libc::WNOHANG) };
        if result_v1 == pid_v1 || (result_v1 < 0 && errno_v1() != libc::EINTR) {
            return;
        }
        if result_v1 == 0 {
            break;
        }
    }
    // SAFETY: WNOHANG confirmed the PID is still this process's live child,
    // so it cannot have been reused before this signal.
    unsafe { libc::kill(pid_v1, libc::SIGKILL) };
    spawn::reap_failed_child_v1(pid_v1);
}

pub(super) fn descriptor_error_v1(operation_v1: &'static str) -> ExplicitFdProcessErrorV1 {
    ExplicitFdProcessErrorV1::DescriptorOperation {
        operation_v1,
        errno_v1: errno_v1(),
    }
}

pub(super) fn spawn_error_v1(operation_v1: &'static str) -> ExplicitFdProcessErrorV1 {
    ExplicitFdProcessErrorV1::Spawn {
        operation_v1,
        errno_v1: errno_v1(),
    }
}

pub(super) fn errno_v1() -> i32 {
    match std::io::Error::last_os_error().raw_os_error() {
        Some(errno_v1) => errno_v1,
        None => libc::EIO,
    }
}
