use super::custody_error_v1;
use crate::{ExplicitFdProcessErrorV1, SpawnedProcessV1};
use std::ffi::{CStr, CString};
use std::mem::MaybeUninit;
use std::os::fd::{AsFd, AsRawFd, BorrowedFd, FromRawFd, OwnedFd};
use std::os::unix::ffi::OsStrExt;

const STDIN_NAME_V1: &[u8] = b"stdin\0";
const STDOUT_NAME_V1: &[u8] = b"stdout\0";
const STDERR_NAME_V1: &[u8] = b"stderr\0";

pub(super) struct DarwinStdioFifosV1 {
    directory_path_v1: CString,
    directory_v1: OwnedFd,
    stdin_path_v1: CString,
    stdout_path_v1: CString,
    stderr_path_v1: CString,
    stdin_guard_v1: Option<OwnedFd>,
    parent_stdin_v1: Option<OwnedFd>,
    parent_stdout_v1: Option<OwnedFd>,
    parent_stderr_v1: Option<OwnedFd>,
}

impl DarwinStdioFifosV1 {
    pub(super) fn try_new_v1() -> Result<Self, ExplicitFdProcessErrorV1> {
        let directory_path_v1 = create_private_directory_v1()?;
        let directory_v1 = open_directory_v1(&directory_path_v1)?;
        let stdin_path_v1 = child_path_v1(&directory_path_v1, "stdin")?;
        let stdout_path_v1 = child_path_v1(&directory_path_v1, "stdout")?;
        let stderr_path_v1 = child_path_v1(&directory_path_v1, "stderr")?;
        let mut fifos_v1 = Self {
            directory_path_v1,
            directory_v1,
            stdin_path_v1,
            stdout_path_v1,
            stderr_path_v1,
            stdin_guard_v1: None,
            parent_stdin_v1: None,
            parent_stdout_v1: None,
            parent_stderr_v1: None,
        };
        fifos_v1.create_entries_v1()?;
        fifos_v1.open_parent_ends_v1()?;
        fifos_v1.freeze_paths_v1()?;
        fifos_v1.verify_before_spawn_v1()?;
        Ok(fifos_v1)
    }

    pub(super) fn stdin_path_v1(&self) -> &CStr {
        &self.stdin_path_v1
    }

    pub(super) fn stdout_path_v1(&self) -> &CStr {
        &self.stdout_path_v1
    }

    pub(super) fn stderr_path_v1(&self) -> &CStr {
        &self.stderr_path_v1
    }

    pub(super) fn finish_after_spawn_v1(
        mut self,
        pid_v1: libc::pid_t,
    ) -> Result<SpawnedProcessV1, ExplicitFdProcessErrorV1> {
        let stdout_v1 =
            self.parent_stdout_v1
                .as_ref()
                .ok_or(ExplicitFdProcessErrorV1::DarwinCustody {
                    operation_v1: "stdout_fifo_missing_v1",
                    errno_v1: libc::EIO,
                })?;
        clear_nonblocking_v1(stdout_v1.as_fd())?;
        let stderr_v1 =
            self.parent_stderr_v1
                .as_ref()
                .ok_or(ExplicitFdProcessErrorV1::DarwinCustody {
                    operation_v1: "stderr_fifo_missing_v1",
                    errno_v1: libc::EIO,
                })?;
        clear_nonblocking_v1(stderr_v1.as_fd())?;
        drop(self.stdin_guard_v1.take());
        let stdin_v1 =
            self.parent_stdin_v1
                .take()
                .ok_or(ExplicitFdProcessErrorV1::DarwinCustody {
                    operation_v1: "stdin_fifo_missing_v1",
                    errno_v1: libc::EIO,
                })?;
        let stdout_v1 =
            self.parent_stdout_v1
                .take()
                .ok_or(ExplicitFdProcessErrorV1::DarwinCustody {
                    operation_v1: "stdout_fifo_missing_v1",
                    errno_v1: libc::EIO,
                })?;
        let stderr_v1 =
            self.parent_stderr_v1
                .take()
                .ok_or(ExplicitFdProcessErrorV1::DarwinCustody {
                    operation_v1: "stderr_fifo_missing_v1",
                    errno_v1: libc::EIO,
                })?;
        Ok(SpawnedProcessV1 {
            pid_v1,
            isolated_process_group_v1: false,
            stdin_v1,
            stdout_v1,
            stderr_v1,
            process_lifetime_guard_v1: None,
        })
    }

    fn create_entries_v1(&mut self) -> Result<(), ExplicitFdProcessErrorV1> {
        for name_v1 in [STDIN_NAME_V1, STDOUT_NAME_V1, STDERR_NAME_V1] {
            // SAFETY: mkfifoat is rooted at the unique private directory and
            // each constant is NUL-terminated with a distinct name.
            if unsafe {
                libc::mkfifoat(self.directory_v1.as_raw_fd(), name_v1.as_ptr().cast(), 0o600)
            } != 0
            {
                return Err(custody_error_v1("mkfifoat_stdio_v1"));
            }
        }
        Ok(())
    }

    fn open_parent_ends_v1(&mut self) -> Result<(), ExplicitFdProcessErrorV1> {
        self.stdin_guard_v1 =
            Some(open_fifo_v1(&self.stdin_path_v1, libc::O_RDONLY | libc::O_NONBLOCK)?);
        self.parent_stdin_v1 = Some(open_fifo_v1(&self.stdin_path_v1, libc::O_WRONLY)?);
        self.parent_stdout_v1 =
            Some(open_fifo_v1(&self.stdout_path_v1, libc::O_RDONLY | libc::O_NONBLOCK)?);
        self.parent_stderr_v1 =
            Some(open_fifo_v1(&self.stderr_path_v1, libc::O_RDONLY | libc::O_NONBLOCK)?);
        Ok(())
    }

    fn freeze_paths_v1(&mut self) -> Result<(), ExplicitFdProcessErrorV1> {
        for path_v1 in [
            &self.stdin_path_v1,
            &self.stdout_path_v1,
            &self.stderr_path_v1,
        ] {
            // SAFETY: chflags targets only entries in the owned private directory.
            if unsafe { libc::chflags(path_v1.as_ptr(), libc::UF_IMMUTABLE) } != 0 {
                return Err(custody_error_v1("chflags_stdio_fifo_v1"));
            }
        }
        // SAFETY: freezing the completed directory prevents path substitution
        // between action construction and posix_spawn.
        if unsafe { libc::fchflags(self.directory_v1.as_raw_fd(), libc::UF_IMMUTABLE) } != 0 {
            return Err(custody_error_v1("fchflags_stdio_directory_v1"));
        }
        // SAFETY: fsync persists the complete FIFO namespace before spawn.
        if unsafe { libc::fsync(self.directory_v1.as_raw_fd()) } != 0 {
            return Err(custody_error_v1("fsync_stdio_directory_v1"));
        }
        Ok(())
    }

    pub(super) fn verify_before_spawn_v1(&self) -> Result<(), ExplicitFdProcessErrorV1> {
        verify_private_directory_binding_v1(&self.directory_path_v1, self.directory_v1.as_fd())?;
        verify_fifo_binding_v1(
            &self.stdin_path_v1,
            self.parent_stdin_v1
                .as_ref()
                .ok_or_else(|| invariant_error_v1("stdin_fifo_missing_during_verify_v1"))?
                .as_fd(),
        )?;
        verify_fifo_binding_v1(
            &self.stdout_path_v1,
            self.parent_stdout_v1
                .as_ref()
                .ok_or_else(|| invariant_error_v1("stdout_fifo_missing_during_verify_v1"))?
                .as_fd(),
        )?;
        verify_fifo_binding_v1(
            &self.stderr_path_v1,
            self.parent_stderr_v1
                .as_ref()
                .ok_or_else(|| invariant_error_v1("stderr_fifo_missing_during_verify_v1"))?
                .as_fd(),
        )
    }
}

impl Drop for DarwinStdioFifosV1 {
    fn drop(&mut self) {
        // BEST-EFFORT: Drop cleanup must not mask the authoritative spawn error.
        // Construction may fail after freezing only a prefix of the namespace,
        // so cleanup always attempts to clear every launcher-owned flag.
        // SAFETY: flags are cleared only on launcher-owned paths.
        unsafe { libc::fchflags(self.directory_v1.as_raw_fd(), 0) };
        for path_v1 in [
            &self.stdin_path_v1,
            &self.stdout_path_v1,
            &self.stderr_path_v1,
        ] {
            // SAFETY: flags are cleared only on launcher-owned paths.
            unsafe { libc::chflags(path_v1.as_ptr(), 0) };
        }
        for name_v1 in [STDIN_NAME_V1, STDOUT_NAME_V1, STDERR_NAME_V1] {
            // SAFETY: unlinkat is rooted at the launcher-owned directory.
            unsafe { libc::unlinkat(self.directory_v1.as_raw_fd(), name_v1.as_ptr().cast(), 0) };
        }
        // SAFETY: directory path is the unique mkdtemp result.
        unsafe { libc::rmdir(self.directory_path_v1.as_ptr()) };
    }
}

fn create_private_directory_v1() -> Result<CString, ExplicitFdProcessErrorV1> {
    let mut template_v1 = std::env::temp_dir();
    template_v1.push("quanta-explicit-fd-stdio.XXXXXX");
    let mut bytes_v1 = template_v1.as_os_str().as_bytes().to_vec();
    bytes_v1.push(0);
    // SAFETY: mkdtemp mutates the writable NUL-terminated template.
    if unsafe { libc::mkdtemp(bytes_v1.as_mut_ptr().cast()) }.is_null() {
        return Err(custody_error_v1("mkdtemp_stdio_v1"));
    }
    bytes_v1.pop();
    CString::new(bytes_v1).map_err(|_| ExplicitFdProcessErrorV1::InvalidCString {
        field_v1: "Darwin stdio directory",
    })
}

fn child_path_v1(directory_v1: &CStr, name_v1: &str) -> Result<CString, ExplicitFdProcessErrorV1> {
    let mut bytes_v1 = directory_v1.to_bytes().to_vec();
    bytes_v1.push(b'/');
    bytes_v1.extend_from_slice(name_v1.as_bytes());
    CString::new(bytes_v1).map_err(|_| ExplicitFdProcessErrorV1::InvalidCString {
        field_v1: "Darwin stdio FIFO path",
    })
}

fn open_directory_v1(path_v1: &CStr) -> Result<OwnedFd, ExplicitFdProcessErrorV1> {
    // SAFETY: open atomically applies CLOEXEC and rejects symlinks.
    let raw_v1 = unsafe {
        libc::open(
            path_v1.as_ptr(),
            libc::O_RDONLY | libc::O_DIRECTORY | libc::O_CLOEXEC | libc::O_NOFOLLOW,
        )
    };
    owned_fd_v1(raw_v1, "open_stdio_directory_v1")
}

fn open_fifo_v1(path_v1: &CStr, flags_v1: i32) -> Result<OwnedFd, ExplicitFdProcessErrorV1> {
    // SAFETY: open atomically applies CLOEXEC; O_NOFOLLOW prevents substitution.
    let raw_v1 =
        unsafe { libc::open(path_v1.as_ptr(), flags_v1 | libc::O_CLOEXEC | libc::O_NOFOLLOW) };
    owned_fd_v1(raw_v1, "open_stdio_fifo_v1")
}

fn owned_fd_v1(
    raw_v1: i32,
    operation_v1: &'static str,
) -> Result<OwnedFd, ExplicitFdProcessErrorV1> {
    if raw_v1 < 0 {
        return Err(custody_error_v1(operation_v1));
    }
    // SAFETY: open returned a fresh descriptor owned here.
    Ok(unsafe { OwnedFd::from_raw_fd(raw_v1) })
}

fn clear_nonblocking_v1(descriptor_v1: BorrowedFd<'_>) -> Result<(), ExplicitFdProcessErrorV1> {
    // SAFETY: F_GETFL reads flags from a live owned descriptor.
    let flags_v1 = unsafe { libc::fcntl(descriptor_v1.as_raw_fd(), libc::F_GETFL) };
    if flags_v1 < 0 {
        return Err(custody_error_v1("fcntl_getfl_stdio_v1"));
    }
    // SAFETY: F_SETFL changes only the non-inheritance status flags; FD_CLOEXEC
    // remains set and is never cleared.
    if unsafe {
        libc::fcntl(descriptor_v1.as_raw_fd(), libc::F_SETFL, flags_v1 & !libc::O_NONBLOCK)
    } != 0
    {
        return Err(custody_error_v1("fcntl_setfl_stdio_v1"));
    }
    Ok(())
}

fn verify_private_directory_binding_v1(
    path_v1: &CStr,
    descriptor_v1: BorrowedFd<'_>,
) -> Result<(), ExplicitFdProcessErrorV1> {
    let descriptor_stat_v1 = fstat_v1(descriptor_v1)?;
    let path_stat_v1 = lstat_v1(path_v1)?;
    // SAFETY: geteuid has no preconditions and observes process credentials.
    let effective_user_v1 = unsafe { libc::geteuid() };
    if !same_node_v1(&descriptor_stat_v1, &path_stat_v1)
        || descriptor_stat_v1.st_mode & libc::S_IFMT != libc::S_IFDIR
        || descriptor_stat_v1.st_mode & 0o777 != 0o700
        || descriptor_stat_v1.st_uid != effective_user_v1
        || descriptor_stat_v1.st_nlink < 2
        || descriptor_stat_v1.st_flags & libc::UF_IMMUTABLE == 0
    {
        return Err(invariant_error_v1("stdio_directory_identity_drift_v1"));
    }
    Ok(())
}

fn verify_fifo_binding_v1(
    path_v1: &CStr,
    descriptor_v1: BorrowedFd<'_>,
) -> Result<(), ExplicitFdProcessErrorV1> {
    let descriptor_stat_v1 = fstat_v1(descriptor_v1)?;
    let path_stat_v1 = lstat_v1(path_v1)?;
    // SAFETY: geteuid has no preconditions and observes process credentials.
    let effective_user_v1 = unsafe { libc::geteuid() };
    if !same_node_v1(&descriptor_stat_v1, &path_stat_v1)
        || descriptor_stat_v1.st_mode & libc::S_IFMT != libc::S_IFIFO
        || descriptor_stat_v1.st_mode & 0o777 != 0o600
        || descriptor_stat_v1.st_uid != effective_user_v1
        || descriptor_stat_v1.st_nlink != 1
        || descriptor_stat_v1.st_flags & libc::UF_IMMUTABLE == 0
    {
        return Err(invariant_error_v1("stdio_fifo_identity_drift_v1"));
    }
    Ok(())
}

fn same_node_v1(left_v1: &libc::stat, right_v1: &libc::stat) -> bool {
    left_v1.st_dev == right_v1.st_dev
        && left_v1.st_ino == right_v1.st_ino
        && left_v1.st_mode == right_v1.st_mode
        && left_v1.st_uid == right_v1.st_uid
        && left_v1.st_nlink == right_v1.st_nlink
        && left_v1.st_flags == right_v1.st_flags
}

fn fstat_v1(descriptor_v1: BorrowedFd<'_>) -> Result<libc::stat, ExplicitFdProcessErrorV1> {
    let mut stat_v1 = MaybeUninit::<libc::stat>::uninit();
    // SAFETY: fstat initializes the pointed-to stat on success.
    if unsafe { libc::fstat(descriptor_v1.as_raw_fd(), stat_v1.as_mut_ptr()) } != 0 {
        return Err(custody_error_v1("fstat_stdio_namespace_v1"));
    }
    // SAFETY: successful fstat initialized the value.
    Ok(unsafe { stat_v1.assume_init() })
}

fn lstat_v1(path_v1: &CStr) -> Result<libc::stat, ExplicitFdProcessErrorV1> {
    let mut stat_v1 = MaybeUninit::<libc::stat>::uninit();
    // SAFETY: lstat initializes the pointed-to stat on success and rejects
    // pathname substitution by comparing the returned inode with the open FD.
    if unsafe { libc::lstat(path_v1.as_ptr(), stat_v1.as_mut_ptr()) } != 0 {
        return Err(custody_error_v1("lstat_stdio_namespace_v1"));
    }
    // SAFETY: successful lstat initialized the value.
    Ok(unsafe { stat_v1.assume_init() })
}

fn invariant_error_v1(operation_v1: &'static str) -> ExplicitFdProcessErrorV1 {
    ExplicitFdProcessErrorV1::DarwinCustody {
        operation_v1,
        errno_v1: libc::EPROTO,
    }
}

#[cfg(test)]
mod tests {
    use super::{open_fifo_v1, verify_fifo_binding_v1};
    use std::ffi::CString;
    use std::os::fd::AsFd;
    use std::os::unix::ffi::OsStrExt;

    #[test]
    fn fifo_path_must_resolve_to_the_open_parent_endpoint_v1() {
        let directory_v1 = tempfile::tempdir().expect("temporary FIFO directory");
        let first_path_v1 = directory_v1.path().join("first");
        let second_path_v1 = directory_v1.path().join("second");
        let first_c_v1 = CString::new(first_path_v1.as_os_str().as_bytes()).expect("first path");
        let second_c_v1 = CString::new(second_path_v1.as_os_str().as_bytes()).expect("second path");
        // SAFETY: both paths are distinct, NUL-terminated, and owned by the test.
        assert_eq!(unsafe { libc::mkfifo(first_c_v1.as_ptr(), 0o600) }, 0);
        // SAFETY: both paths are distinct, NUL-terminated, and owned by the test.
        assert_eq!(unsafe { libc::mkfifo(second_c_v1.as_ptr(), 0o600) }, 0);
        let second_v1 = open_fifo_v1(&second_c_v1, libc::O_RDWR).expect("open second FIFO");
        // SAFETY: immutable flags are set only on the two test-owned FIFOs.
        assert_eq!(unsafe { libc::chflags(first_c_v1.as_ptr(), libc::UF_IMMUTABLE) }, 0);
        // SAFETY: immutable flags are set only on the two test-owned FIFOs.
        assert_eq!(unsafe { libc::chflags(second_c_v1.as_ptr(), libc::UF_IMMUTABLE) }, 0);
        verify_fifo_binding_v1(&second_c_v1, second_v1.as_fd())
            .expect("matching immutable FIFO binding");
        assert!(verify_fifo_binding_v1(&first_c_v1, second_v1.as_fd()).is_err());
        // SAFETY: cleanup clears flags only on the two test-owned FIFOs.
        unsafe { libc::chflags(first_c_v1.as_ptr(), 0) };
        // SAFETY: cleanup clears flags only on the two test-owned FIFOs.
        unsafe { libc::chflags(second_c_v1.as_ptr(), 0) };
    }
}
