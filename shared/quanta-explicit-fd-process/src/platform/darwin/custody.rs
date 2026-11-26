mod custody_io;
mod launch_image;

use super::{custody_error_v1, descriptor_identity_v1, DescriptorIdentityV1};
use crate::ExplicitFdProcessErrorV1;
use custody_io::{copy_exact_bytes_v1, sync_descriptor_v1, verify_custody_sha256_v1};
use std::ffi::{CStr, CString, OsString};
use std::mem::MaybeUninit;
use std::os::fd::{AsFd, AsRawFd, BorrowedFd, FromRawFd, OwnedFd};
use std::os::unix::ffi::{OsStrExt, OsStringExt};
use std::path::PathBuf;

pub(crate) use launch_image::DarwinEphemeralLaunchImageV1;

const CUSTODY_FILE_NAME_V1: &[u8] = b"image\0";

#[derive(Clone, Copy, Debug)]
pub(crate) enum DarwinCustodyKindV1 {
    Executable,
    ReadOnlyFile,
}

impl DarwinCustodyKindV1 {
    const fn mode_v1(self) -> libc::mode_t {
        match self {
            // Executables remain immutable data while pinned. Darwin dyld can
            // hang in an uninterruptible state when asked to execute a file
            // carrying UF_IMMUTABLE, so spawn uses a separate ephemeral image.
            Self::Executable => 0o400,
            Self::ReadOnlyFile => 0o400,
        }
    }
}

#[derive(Debug)]
pub(crate) struct DarwinImmutableCustodyV1 {
    directory_path_v1: CString,
    file_path_v1: CString,
    directory_v1: OwnedFd,
    file_v1: OwnedFd,
    directory_identity_v1: DescriptorIdentityV1,
    file_identity_v1: DescriptorIdentityV1,
    mode_v1: libc::mode_t,
}

impl DarwinImmutableCustodyV1 {
    pub(crate) fn try_copy_v1(
        source_v1: BorrowedFd<'_>,
        size_v1: u64,
        kind_v1: DarwinCustodyKindV1,
        expected_sha256_v1: &str,
    ) -> Result<Self, ExplicitFdProcessErrorV1> {
        let directory_path_v1 = create_private_directory_v1()?;
        let file_path_v1 = custody_file_path_v1(&directory_path_v1)?;
        let mut guard_v1 = CustodyBuildGuardV1::new_v1(directory_path_v1.clone());
        guard_v1.open_directory_v1()?;
        guard_v1.copy_and_freeze_file_v1(source_v1, size_v1, kind_v1.mode_v1())?;
        let file_identity_v1 =
            guard_v1.reopen_and_verify_file_v1(size_v1, kind_v1.mode_v1(), expected_sha256_v1)?;
        let directory_identity_v1 = guard_v1.freeze_directory_v1()?;
        let (directory_v1, file_v1) = guard_v1.disarm_v1()?;
        let custody_v1 = Self {
            directory_path_v1,
            file_path_v1,
            directory_v1,
            file_v1,
            directory_identity_v1,
            file_identity_v1,
            mode_v1: kind_v1.mode_v1(),
        };
        custody_v1.verify_v1()?;
        Ok(custody_v1)
    }

    pub(crate) fn descriptor_v1(&self) -> BorrowedFd<'_> {
        self.file_v1.as_fd()
    }

    pub(crate) fn size_v1(&self) -> u64 {
        self.file_identity_v1.size_v1
    }

    pub(crate) fn verify_v1(&self) -> Result<(), ExplicitFdProcessErrorV1> {
        if descriptor_identity_v1(self.directory_v1.as_fd())? != self.directory_identity_v1
            || descriptor_identity_v1(self.file_v1.as_fd())? != self.file_identity_v1
            || path_identity_v1(&self.directory_path_v1)? != self.directory_identity_v1
            || path_identity_v1(&self.file_path_v1)? != self.file_identity_v1
            || self.file_identity_v1.size_v1 == 0
            || self.file_identity_v1.mode_v1 & 0o777 != u32::from(self.mode_v1)
        {
            return Err(ExplicitFdProcessErrorV1::ExecutableIdentityDrift);
        }
        verify_immutable_flag_v1(self.directory_v1.as_fd())?;
        verify_immutable_flag_v1(self.file_v1.as_fd())
    }
}

impl Drop for DarwinImmutableCustodyV1 {
    fn drop(&mut self) {
        // BEST-EFFORT: immutable private custody is safe to retain if cleanup
        // fails during Drop; descriptors still close normally.
        cleanup_custody_v1(
            &self.directory_path_v1,
            self.directory_v1.as_fd(),
            Some(self.file_v1.as_fd()),
            true,
            true,
        );
    }
}

#[derive(Debug)]
struct CustodyBuildGuardV1 {
    directory_path_v1: CString,
    directory_v1: Option<OwnedFd>,
    file_v1: Option<OwnedFd>,
    file_created_v1: bool,
    file_immutable_v1: bool,
    directory_immutable_v1: bool,
    armed_v1: bool,
}

impl CustodyBuildGuardV1 {
    fn new_v1(directory_path_v1: CString) -> Self {
        Self {
            directory_path_v1,
            directory_v1: None,
            file_v1: None,
            file_created_v1: false,
            file_immutable_v1: false,
            directory_immutable_v1: false,
            armed_v1: true,
        }
    }

    fn open_directory_v1(&mut self) -> Result<(), ExplicitFdProcessErrorV1> {
        self.directory_v1 = Some(open_directory_v1(&self.directory_path_v1)?);
        Ok(())
    }

    fn copy_and_freeze_file_v1(
        &mut self,
        source_v1: BorrowedFd<'_>,
        size_v1: u64,
        mode_v1: libc::mode_t,
    ) -> Result<(), ExplicitFdProcessErrorV1> {
        self.file_v1 = Some(create_custody_file_v1(self.directory_descriptor_v1()?)?);
        self.file_created_v1 = true;
        let file_v1 = self.file_descriptor_v1()?;
        copy_exact_bytes_v1(source_v1, file_v1, size_v1)?;
        sync_descriptor_v1(file_v1, "fsync_custody_bytes_v1")?;
        // SAFETY: fchmod mutates only the private custody file.
        if unsafe { libc::fchmod(file_v1.as_raw_fd(), mode_v1) } != 0 {
            return Err(custody_error_v1("fchmod_custody_v1"));
        }
        sync_descriptor_v1(file_v1, "fsync_custody_mode_v1")?;
        // SAFETY: immutable flags bind the completed private snapshot.
        if unsafe { libc::fchflags(file_v1.as_raw_fd(), libc::UF_IMMUTABLE) } != 0 {
            return Err(custody_error_v1("fchflags_custody_file_v1"));
        }
        self.file_immutable_v1 = true;
        sync_descriptor_v1(self.file_descriptor_v1()?, "fsync_custody_immutable_v1")
    }

    fn reopen_and_verify_file_v1(
        &mut self,
        size_v1: u64,
        mode_v1: libc::mode_t,
        expected_sha256_v1: &str,
    ) -> Result<DescriptorIdentityV1, ExplicitFdProcessErrorV1> {
        let read_file_v1 = open_custody_file_v1(self.directory_descriptor_v1()?)?;
        let write_identity_v1 = descriptor_identity_v1(self.file_descriptor_v1()?)?;
        let file_identity_v1 = descriptor_identity_v1(read_file_v1.as_fd())?;
        if write_identity_v1 != file_identity_v1
            || file_identity_v1.size_v1 != size_v1
            || file_identity_v1.mode_v1 & 0o777 != u32::from(mode_v1)
        {
            return Err(ExplicitFdProcessErrorV1::ExecutableIdentityDrift);
        }
        verify_custody_sha256_v1(read_file_v1.as_fd(), size_v1, expected_sha256_v1)?;
        drop(self.file_v1.take());
        self.file_v1 = Some(read_file_v1);
        Ok(file_identity_v1)
    }

    fn freeze_directory_v1(&mut self) -> Result<DescriptorIdentityV1, ExplicitFdProcessErrorV1> {
        let directory_v1 = self.directory_descriptor_v1()?;
        sync_descriptor_v1(directory_v1, "fsync_custody_directory_v1")?;
        // SAFETY: the complete private directory contains one immutable file.
        if unsafe { libc::fchflags(directory_v1.as_raw_fd(), libc::UF_IMMUTABLE) } != 0 {
            return Err(custody_error_v1("fchflags_custody_directory_v1"));
        }
        self.directory_immutable_v1 = true;
        let directory_v1 = self.directory_descriptor_v1()?;
        sync_descriptor_v1(directory_v1, "fsync_custody_directory_flags_v1")?;
        descriptor_identity_v1(directory_v1)
    }

    fn directory_descriptor_v1(&self) -> Result<BorrowedFd<'_>, ExplicitFdProcessErrorV1> {
        self.directory_v1
            .as_ref()
            .map(|directory_v1| directory_v1.as_fd())
            .ok_or(ExplicitFdProcessErrorV1::DarwinCustody {
                operation_v1: "custody_directory_missing_v1",
                errno_v1: libc::EIO,
            })
    }

    fn file_descriptor_v1(&self) -> Result<BorrowedFd<'_>, ExplicitFdProcessErrorV1> {
        self.file_v1.as_ref().map(|file_v1| file_v1.as_fd()).ok_or(
            ExplicitFdProcessErrorV1::DarwinCustody {
                operation_v1: "custody_file_missing_v1",
                errno_v1: libc::EIO,
            },
        )
    }

    fn disarm_v1(&mut self) -> Result<(OwnedFd, OwnedFd), ExplicitFdProcessErrorV1> {
        let directory_v1 =
            self.directory_v1
                .take()
                .ok_or(ExplicitFdProcessErrorV1::DarwinCustody {
                    operation_v1: "custody_directory_missing_v1",
                    errno_v1: libc::EIO,
                })?;
        let file_v1 = self
            .file_v1
            .take()
            .ok_or(ExplicitFdProcessErrorV1::DarwinCustody {
                operation_v1: "custody_file_missing_v1",
                errno_v1: libc::EIO,
            })?;
        self.armed_v1 = false;
        Ok((directory_v1, file_v1))
    }
}

impl Drop for CustodyBuildGuardV1 {
    fn drop(&mut self) {
        if !self.armed_v1 {
            return;
        }
        // BEST-EFFORT: construction failure must attempt to remove all private
        // custody residue without masking the authoritative construction error.
        if let Some(directory_v1) = &self.directory_v1 {
            cleanup_custody_v1(
                &self.directory_path_v1,
                directory_v1.as_fd(),
                self.file_v1.as_ref().map(|file_v1| file_v1.as_fd()),
                self.file_created_v1 && self.file_immutable_v1,
                self.directory_immutable_v1,
            );
        } else {
            // SAFETY: path is the unique directory returned by mkdtemp.
            unsafe { libc::rmdir(self.directory_path_v1.as_ptr()) };
        }
    }
}

fn create_private_directory_v1() -> Result<CString, ExplicitFdProcessErrorV1> {
    let mut template_v1 = std::env::temp_dir();
    template_v1.push("quanta-explicit-fd.XXXXXX");
    let mut bytes_v1 = template_v1.as_os_str().as_bytes().to_vec();
    bytes_v1.push(0);
    // SAFETY: mkdtemp mutates the writable NUL-terminated template buffer.
    let result_v1 = unsafe { libc::mkdtemp(bytes_v1.as_mut_ptr().cast()) };
    if result_v1.is_null() {
        return Err(custody_error_v1("mkdtemp_custody_v1"));
    }
    bytes_v1.pop();
    CString::new(bytes_v1).map_err(|_| ExplicitFdProcessErrorV1::InvalidCString {
        field_v1: "Darwin custody path",
    })
}

fn custody_file_path_v1(directory_v1: &CStr) -> Result<CString, ExplicitFdProcessErrorV1> {
    let mut path_v1 = PathBuf::from(OsString::from_vec(directory_v1.to_bytes().to_vec()));
    path_v1.push("image");
    CString::new(path_v1.as_os_str().as_bytes()).map_err(|_| {
        ExplicitFdProcessErrorV1::InvalidCString {
            field_v1: "Darwin custody file path",
        }
    })
}

fn open_directory_v1(path_v1: &CStr) -> Result<OwnedFd, ExplicitFdProcessErrorV1> {
    // SAFETY: path is NUL-terminated and open atomically applies CLOEXEC.
    let raw_v1 = unsafe {
        libc::open(
            path_v1.as_ptr(),
            libc::O_RDONLY | libc::O_DIRECTORY | libc::O_CLOEXEC | libc::O_NOFOLLOW,
        )
    };
    owned_fd_v1(raw_v1, "open_custody_directory_v1")
}

fn create_custody_file_v1(
    directory_v1: BorrowedFd<'_>,
) -> Result<OwnedFd, ExplicitFdProcessErrorV1> {
    // SAFETY: openat is rooted at the private directory and O_EXCL/O_NOFOLLOW
    // prevent reuse or symlink substitution.
    let raw_v1 = unsafe {
        libc::openat(
            directory_v1.as_raw_fd(),
            CUSTODY_FILE_NAME_V1.as_ptr().cast(),
            libc::O_WRONLY | libc::O_CREAT | libc::O_EXCL | libc::O_CLOEXEC | libc::O_NOFOLLOW,
            0o600,
        )
    };
    owned_fd_v1(raw_v1, "openat_create_custody_file_v1")
}

fn open_custody_file_v1(directory_v1: BorrowedFd<'_>) -> Result<OwnedFd, ExplicitFdProcessErrorV1> {
    // SAFETY: the private immutable entry is opened read-only with CLOEXEC.
    let raw_v1 = unsafe {
        libc::openat(
            directory_v1.as_raw_fd(),
            CUSTODY_FILE_NAME_V1.as_ptr().cast(),
            libc::O_RDONLY | libc::O_CLOEXEC | libc::O_NOFOLLOW,
        )
    };
    owned_fd_v1(raw_v1, "openat_read_custody_file_v1")
}

fn owned_fd_v1(
    raw_v1: i32,
    operation_v1: &'static str,
) -> Result<OwnedFd, ExplicitFdProcessErrorV1> {
    if raw_v1 < 0 {
        return Err(custody_error_v1(operation_v1));
    }
    // SAFETY: successful open/openat returned a fresh descriptor.
    Ok(unsafe { OwnedFd::from_raw_fd(raw_v1) })
}

fn verify_immutable_flag_v1(descriptor_v1: BorrowedFd<'_>) -> Result<(), ExplicitFdProcessErrorV1> {
    let mut stat_v1 = MaybeUninit::<libc::stat>::uninit();
    // SAFETY: fstat initializes stat on success.
    if unsafe { libc::fstat(descriptor_v1.as_raw_fd(), stat_v1.as_mut_ptr()) } != 0 {
        return Err(custody_error_v1("fstat_custody_flags_v1"));
    }
    // SAFETY: successful fstat initialized the value.
    if unsafe { stat_v1.assume_init() }.st_flags & libc::UF_IMMUTABLE == 0 {
        return Err(ExplicitFdProcessErrorV1::ExecutableIdentityDrift);
    }
    Ok(())
}

fn path_identity_v1(path_v1: &CStr) -> Result<DescriptorIdentityV1, ExplicitFdProcessErrorV1> {
    let mut stat_v1 = MaybeUninit::<libc::stat>::uninit();
    // SAFETY: lstat initializes stat on success and does not follow symlinks.
    if unsafe { libc::lstat(path_v1.as_ptr(), stat_v1.as_mut_ptr()) } != 0 {
        return Err(custody_error_v1("lstat_custody_v1"));
    }
    // SAFETY: successful lstat initialized the value.
    let stat_v1 = unsafe { stat_v1.assume_init() };
    Ok(DescriptorIdentityV1 {
        device_v1: u64::try_from(stat_v1.st_dev)
            .map_err(|_| ExplicitFdProcessErrorV1::ExecutableIdentityDrift)?,
        inode_v1: stat_v1.st_ino,
        size_v1: u64::try_from(stat_v1.st_size)
            .map_err(|_| ExplicitFdProcessErrorV1::ExecutableIdentityDrift)?,
        mode_v1: u32::from(stat_v1.st_mode),
        modified_seconds_v1: stat_v1.st_mtime,
        modified_nanoseconds_v1: stat_v1.st_mtime_nsec,
        changed_seconds_v1: stat_v1.st_ctime,
        changed_nanoseconds_v1: stat_v1.st_ctime_nsec,
        regular_file_v1: stat_v1.st_mode & libc::S_IFMT == libc::S_IFREG,
        directory_v1: stat_v1.st_mode & libc::S_IFMT == libc::S_IFDIR,
    })
}

fn cleanup_custody_v1(
    directory_path_v1: &CStr,
    directory_v1: BorrowedFd<'_>,
    file_v1: Option<BorrowedFd<'_>>,
    file_immutable_v1: bool,
    directory_immutable_v1: bool,
) {
    if directory_immutable_v1 {
        // SAFETY: best-effort cleanup clears flags only on owned custody.
        unsafe { libc::fchflags(directory_v1.as_raw_fd(), 0) };
    }
    if file_immutable_v1 {
        if let Some(file_v1) = file_v1 {
            // SAFETY: best-effort cleanup clears flags only on owned custody.
            unsafe { libc::fchflags(file_v1.as_raw_fd(), 0) };
        }
    }
    // SAFETY: unlinkat is rooted at the owned private directory.
    unsafe {
        libc::unlinkat(
            directory_v1.as_raw_fd(),
            CUSTODY_FILE_NAME_V1.as_ptr().cast(),
            0,
        )
    };
    // SAFETY: directory path is the unique mkdtemp result owned by custody.
    unsafe { libc::rmdir(directory_path_v1.as_ptr()) };
}
