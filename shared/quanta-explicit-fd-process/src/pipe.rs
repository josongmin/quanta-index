use crate::ExplicitFdProcessErrorV1;
#[cfg(target_os = "macos")]
use std::os::fd::AsRawFd;
use std::os::fd::{FromRawFd, OwnedFd, RawFd};

/// Create an unnamed pipe whose parent-owned endpoints are close-on-exec.
///
/// Linux applies `O_CLOEXEC` atomically with `pipe2`. Darwin has no `pipe2`,
/// so the flags are installed immediately after `pipe`; that platform cannot
/// eliminate the process-wide fork/exec race in this constructor alone. The
/// Darwin launcher in this crate additionally uses
/// `POSIX_SPAWN_CLOEXEC_DEFAULT`, so descriptors not named by its file actions
/// cannot leak into children launched through this authority.
pub fn try_create_cloexec_pipe_v1() -> Result<(OwnedFd, OwnedFd), ExplicitFdProcessErrorV1> {
    let [read_v1, write_v1] = create_pipe_v1()?;
    // SAFETY: a successful pipe/pipe2 call returns two new, uniquely owned
    // descriptors. Wrapping both immediately gives every later error path RAII
    // cleanup.
    let read_v1 = unsafe { OwnedFd::from_raw_fd(read_v1) };
    // SAFETY: see the ownership argument above for the second returned end.
    let write_v1 = unsafe { OwnedFd::from_raw_fd(write_v1) };
    #[cfg(target_os = "macos")]
    {
        set_close_on_exec_v1(read_v1.as_raw_fd())?;
        set_close_on_exec_v1(write_v1.as_raw_fd())?;
    }
    Ok((read_v1, write_v1))
}

#[cfg(target_os = "linux")]
fn create_pipe_v1() -> Result<[RawFd; 2], ExplicitFdProcessErrorV1> {
    let mut descriptors_v1 = [-1; 2];
    // SAFETY: descriptors_v1 points to writable storage for exactly two fds.
    let result_v1 = unsafe { libc::pipe2(descriptors_v1.as_mut_ptr(), libc::O_CLOEXEC) };
    syscall_result_v1(result_v1, "pipe2_cloexec_v1")?;
    Ok(descriptors_v1)
}

#[cfg(target_os = "macos")]
fn create_pipe_v1() -> Result<[RawFd; 2], ExplicitFdProcessErrorV1> {
    let mut descriptors_v1 = [-1; 2];
    // SAFETY: descriptors_v1 points to writable storage for exactly two fds.
    let result_v1 = unsafe { libc::pipe(descriptors_v1.as_mut_ptr()) };
    syscall_result_v1(result_v1, "pipe_v1")?;
    Ok(descriptors_v1)
}

#[cfg(target_os = "macos")]
fn set_close_on_exec_v1(descriptor_v1: RawFd) -> Result<(), ExplicitFdProcessErrorV1> {
    // SAFETY: descriptor_v1 is a live owned pipe endpoint.
    let flags_v1 = unsafe { libc::fcntl(descriptor_v1, libc::F_GETFD) };
    syscall_result_v1(flags_v1, "fcntl_getfd_pipe_v1")?;
    // SAFETY: descriptor_v1 is live and the third argument is the documented
    // integer flag set for F_SETFD.
    let result_v1 =
        unsafe { libc::fcntl(descriptor_v1, libc::F_SETFD, flags_v1 | libc::FD_CLOEXEC) };
    syscall_result_v1(result_v1, "fcntl_setfd_cloexec_pipe_v1")
}

fn syscall_result_v1(
    result_v1: libc::c_int,
    operation_v1: &'static str,
) -> Result<(), ExplicitFdProcessErrorV1> {
    if result_v1 == -1 {
        return Err(ExplicitFdProcessErrorV1::DescriptorOperation {
            operation_v1,
            errno_v1: std::io::Error::last_os_error()
                .raw_os_error()
                .unwrap_or(libc::EIO),
        });
    }
    Ok(())
}
