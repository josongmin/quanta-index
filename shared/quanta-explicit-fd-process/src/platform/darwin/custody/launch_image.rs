use super::custody_io::{copy_exact_bytes_v1, sync_descriptor_v1, verify_custody_sha256_v1};
use super::path_identity_v1;
use crate::platform::darwin::{custody_error_v1, descriptor_identity_v1, DescriptorIdentityV1};
use crate::ExplicitFdProcessErrorV1;
use std::ffi::{CStr, CString};
use std::mem::MaybeUninit;
use std::os::fd::{AsFd, AsRawFd, BorrowedFd, FromRawFd, OwnedFd};
use std::os::unix::ffi::OsStrExt;

const LAUNCH_FILE_NAME_V1: &[u8] = b"executable\0";

/// Per-spawn executable path derived from immutable descriptor custody.
///
/// Both the directory and executable must remain free of immutable flags for
/// Darwin dyld. The random 0700 directory, owner/link/mode checks, exact inode
/// binding, and an immediate digest verification reject accidental drift and
/// cross-UID namespace replacement. Darwin exposes no `fexecve`/`execveat`, so
/// this adapter does not claim resistance to an actively malicious same-UID
/// process racing the final verification-to-`posix_spawn` syscall gap. The
/// child handle owns this namespace until successful reap or kill-and-reap;
/// `posix_spawn` returning does not prove dyld has finished pathname loading.
#[derive(Debug)]
pub(crate) struct DarwinEphemeralLaunchImageV1 {
    directory_path_v1: CString,
    file_path_v1: CString,
    directory_v1: OwnedFd,
    file_v1: OwnedFd,
    directory_identity_v1: DescriptorIdentityV1,
    file_identity_v1: DescriptorIdentityV1,
    expected_sha256_v1: String,
}

impl DarwinEphemeralLaunchImageV1 {
    pub(crate) fn try_copy_v1(
        source_v1: BorrowedFd<'_>,
        size_v1: u64,
        expected_sha256_v1: &str,
    ) -> Result<Self, ExplicitFdProcessErrorV1> {
        let directory_path_v1 = create_launch_directory_v1()?;
        let file_path_v1 = launch_file_path_v1(&directory_path_v1)?;
        let mut guard_v1 = LaunchImageBuildGuardV1::new_v1(directory_path_v1.clone());
        guard_v1.open_directory_v1()?;
        guard_v1.copy_and_finalize_file_v1(source_v1, size_v1)?;
        let file_identity_v1 = guard_v1.reopen_and_verify_file_v1(size_v1, expected_sha256_v1)?;
        let directory_identity_v1 = guard_v1.finalize_directory_v1()?;
        let (directory_v1, file_v1) = guard_v1.disarm_v1()?;
        let launch_v1 = Self {
            directory_path_v1,
            file_path_v1,
            directory_v1,
            file_v1,
            directory_identity_v1,
            file_identity_v1,
            expected_sha256_v1: expected_sha256_v1.to_owned(),
        };
        launch_v1.verify_before_spawn_v1()?;
        Ok(launch_v1)
    }

    pub(crate) fn executable_path_v1(&self) -> &CStr {
        &self.file_path_v1
    }

    pub(crate) fn verify_before_spawn_v1(&self) -> Result<(), ExplicitFdProcessErrorV1> {
        if descriptor_identity_v1(self.directory_v1.as_fd())? != self.directory_identity_v1
            || descriptor_identity_v1(self.file_v1.as_fd())? != self.file_identity_v1
            || path_identity_v1(&self.directory_path_v1)? != self.directory_identity_v1
            || path_identity_v1(&self.file_path_v1)? != self.file_identity_v1
            || !self.file_identity_v1.regular_file_v1
            || self.file_identity_v1.size_v1 == 0
            || self.file_identity_v1.mode_v1 & 0o777 != 0o500
        {
            return Err(ExplicitFdProcessErrorV1::ExecutableIdentityDrift);
        }
        verify_launch_directory_v1(self.directory_v1.as_fd())?;
        verify_launch_file_metadata_v1(self.file_v1.as_fd())?;
        verify_custody_sha256_v1(
            self.file_v1.as_fd(),
            self.file_identity_v1.size_v1,
            &self.expected_sha256_v1,
        )
    }
}

impl Drop for DarwinEphemeralLaunchImageV1 {
    fn drop(&mut self) {
        cleanup_launch_image_v1(
            &self.directory_path_v1,
            self.directory_v1.as_fd(),
            Some(self.file_v1.as_fd()),
            true,
        );
    }
}

struct LaunchImageBuildGuardV1 {
    directory_path_v1: CString,
    directory_v1: Option<OwnedFd>,
    file_v1: Option<OwnedFd>,
    file_created_v1: bool,
    armed_v1: bool,
}

impl LaunchImageBuildGuardV1 {
    fn new_v1(directory_path_v1: CString) -> Self {
        Self {
            directory_path_v1,
            directory_v1: None,
            file_v1: None,
            file_created_v1: false,
            armed_v1: true,
        }
    }

    fn open_directory_v1(&mut self) -> Result<(), ExplicitFdProcessErrorV1> {
        self.directory_v1 = Some(open_launch_directory_v1(&self.directory_path_v1)?);
        Ok(())
    }

    fn copy_and_finalize_file_v1(
        &mut self,
        source_v1: BorrowedFd<'_>,
        size_v1: u64,
    ) -> Result<(), ExplicitFdProcessErrorV1> {
        self.file_v1 = Some(create_launch_file_v1(self.directory_descriptor_v1()?)?);
        self.file_created_v1 = true;
        let file_v1 = self.file_descriptor_v1()?;
        copy_exact_bytes_v1(source_v1, file_v1, size_v1)?;
        sync_descriptor_v1(file_v1, "fsync_launch_image_bytes_v1")?;
        // SAFETY: fchmod mutates only the private per-spawn image.
        if unsafe { libc::fchmod(file_v1.as_raw_fd(), 0o500) } != 0 {
            return Err(custody_error_v1("fchmod_launch_image_v1"));
        }
        sync_descriptor_v1(file_v1, "fsync_launch_image_mode_v1")
    }

    fn reopen_and_verify_file_v1(
        &mut self,
        size_v1: u64,
        expected_sha256_v1: &str,
    ) -> Result<DescriptorIdentityV1, ExplicitFdProcessErrorV1> {
        let read_file_v1 = open_launch_file_v1(self.directory_descriptor_v1()?)?;
        let write_identity_v1 = descriptor_identity_v1(self.file_descriptor_v1()?)?;
        let file_identity_v1 = descriptor_identity_v1(read_file_v1.as_fd())?;
        if write_identity_v1 != file_identity_v1
            || !file_identity_v1.regular_file_v1
            || file_identity_v1.size_v1 != size_v1
            || file_identity_v1.mode_v1 & 0o777 != 0o500
        {
            return Err(ExplicitFdProcessErrorV1::ExecutableIdentityDrift);
        }
        verify_launch_file_metadata_v1(read_file_v1.as_fd())?;
        verify_custody_sha256_v1(read_file_v1.as_fd(), size_v1, expected_sha256_v1)?;
        drop(self.file_v1.take());
        self.file_v1 = Some(read_file_v1);
        Ok(file_identity_v1)
    }

    fn finalize_directory_v1(&mut self) -> Result<DescriptorIdentityV1, ExplicitFdProcessErrorV1> {
        let directory_v1 = self.directory_descriptor_v1()?;
        sync_descriptor_v1(directory_v1, "fsync_launch_directory_v1")?;
        verify_launch_directory_v1(directory_v1)?;
        descriptor_identity_v1(directory_v1)
    }

    fn directory_descriptor_v1(&self) -> Result<BorrowedFd<'_>, ExplicitFdProcessErrorV1> {
        self.directory_v1
            .as_ref()
            .map(|directory_v1| directory_v1.as_fd())
            .ok_or_else(|| launch_error_v1("launch_directory_missing_v1"))
    }

    fn file_descriptor_v1(&self) -> Result<BorrowedFd<'_>, ExplicitFdProcessErrorV1> {
        self.file_v1
            .as_ref()
            .map(|file_v1| file_v1.as_fd())
            .ok_or_else(|| launch_error_v1("launch_file_missing_v1"))
    }

    fn disarm_v1(&mut self) -> Result<(OwnedFd, OwnedFd), ExplicitFdProcessErrorV1> {
        let directory_v1 = self
            .directory_v1
            .take()
            .ok_or_else(|| launch_error_v1("launch_directory_missing_v1"))?;
        let file_v1 = self
            .file_v1
            .take()
            .ok_or_else(|| launch_error_v1("launch_file_missing_v1"))?;
        self.armed_v1 = false;
        Ok((directory_v1, file_v1))
    }
}

impl Drop for LaunchImageBuildGuardV1 {
    fn drop(&mut self) {
        if !self.armed_v1 {
            return;
        }
        if let Some(directory_v1) = &self.directory_v1 {
            cleanup_launch_image_v1(
                &self.directory_path_v1,
                directory_v1.as_fd(),
                self.file_v1.as_ref().map(|file_v1| file_v1.as_fd()),
                self.file_created_v1,
            );
        } else {
            // SAFETY: path is the unique directory returned by mkdtemp.
            unsafe { libc::rmdir(self.directory_path_v1.as_ptr()) };
        }
    }
}

fn create_launch_directory_v1() -> Result<CString, ExplicitFdProcessErrorV1> {
    let mut template_v1 = std::env::temp_dir();
    template_v1.push("quanta-explicit-fd-launch.XXXXXX");
    let mut bytes_v1 = template_v1.as_os_str().as_bytes().to_vec();
    bytes_v1.push(0);
    // SAFETY: mkdtemp mutates the writable NUL-terminated template.
    if unsafe { libc::mkdtemp(bytes_v1.as_mut_ptr().cast()) }.is_null() {
        return Err(custody_error_v1("mkdtemp_launch_image_v1"));
    }
    bytes_v1.pop();
    CString::new(bytes_v1).map_err(|_| ExplicitFdProcessErrorV1::InvalidCString {
        field_v1: "Darwin launch image directory",
    })
}

fn launch_file_path_v1(directory_v1: &CStr) -> Result<CString, ExplicitFdProcessErrorV1> {
    let mut bytes_v1 = directory_v1.to_bytes().to_vec();
    bytes_v1.extend_from_slice(b"/executable");
    CString::new(bytes_v1).map_err(|_| ExplicitFdProcessErrorV1::InvalidCString {
        field_v1: "Darwin launch image path",
    })
}

fn open_launch_directory_v1(path_v1: &CStr) -> Result<OwnedFd, ExplicitFdProcessErrorV1> {
    // SAFETY: open atomically applies CLOEXEC and rejects symlinks.
    let raw_v1 = unsafe {
        libc::open(
            path_v1.as_ptr(),
            libc::O_RDONLY | libc::O_DIRECTORY | libc::O_CLOEXEC | libc::O_NOFOLLOW,
        )
    };
    owned_fd_v1(raw_v1, "open_launch_directory_v1")
}

fn create_launch_file_v1(
    directory_v1: BorrowedFd<'_>,
) -> Result<OwnedFd, ExplicitFdProcessErrorV1> {
    // SAFETY: openat is rooted at the private directory and rejects reuse.
    let raw_v1 = unsafe {
        libc::openat(
            directory_v1.as_raw_fd(),
            LAUNCH_FILE_NAME_V1.as_ptr().cast(),
            libc::O_WRONLY | libc::O_CREAT | libc::O_EXCL | libc::O_CLOEXEC | libc::O_NOFOLLOW,
            0o600,
        )
    };
    owned_fd_v1(raw_v1, "openat_create_launch_image_v1")
}

fn open_launch_file_v1(directory_v1: BorrowedFd<'_>) -> Result<OwnedFd, ExplicitFdProcessErrorV1> {
    // SAFETY: the finalized private entry is opened read-only with CLOEXEC.
    let raw_v1 = unsafe {
        libc::openat(
            directory_v1.as_raw_fd(),
            LAUNCH_FILE_NAME_V1.as_ptr().cast(),
            libc::O_RDONLY | libc::O_CLOEXEC | libc::O_NOFOLLOW,
        )
    };
    owned_fd_v1(raw_v1, "openat_read_launch_image_v1")
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

fn verify_launch_directory_v1(
    descriptor_v1: BorrowedFd<'_>,
) -> Result<(), ExplicitFdProcessErrorV1> {
    let stat_v1 = fstat_launch_v1(descriptor_v1, "fstat_launch_directory_v1")?;
    // SAFETY: geteuid has no preconditions and observes process credentials.
    let effective_user_v1 = unsafe { libc::geteuid() };
    if stat_v1.st_mode & libc::S_IFMT != libc::S_IFDIR
        || stat_v1.st_mode & 0o777 != 0o700
        || stat_v1.st_uid != effective_user_v1
        || stat_v1.st_nlink < 2
        || stat_v1.st_flags & (libc::UF_IMMUTABLE | libc::SF_IMMUTABLE) != 0
    {
        return Err(ExplicitFdProcessErrorV1::ExecutableIdentityDrift);
    }
    Ok(())
}

fn verify_launch_file_metadata_v1(
    descriptor_v1: BorrowedFd<'_>,
) -> Result<(), ExplicitFdProcessErrorV1> {
    let stat_v1 = fstat_launch_v1(descriptor_v1, "fstat_launch_image_v1")?;
    // SAFETY: geteuid has no preconditions and observes process credentials.
    let effective_user_v1 = unsafe { libc::geteuid() };
    if stat_v1.st_mode & libc::S_IFMT != libc::S_IFREG
        || stat_v1.st_mode & 0o777 != 0o500
        || stat_v1.st_uid != effective_user_v1
        || stat_v1.st_nlink != 1
        || stat_v1.st_flags & (libc::UF_IMMUTABLE | libc::SF_IMMUTABLE) != 0
    {
        return Err(ExplicitFdProcessErrorV1::ExecutableIdentityDrift);
    }
    Ok(())
}

fn fstat_launch_v1(
    descriptor_v1: BorrowedFd<'_>,
    operation_v1: &'static str,
) -> Result<libc::stat, ExplicitFdProcessErrorV1> {
    let mut stat_v1 = MaybeUninit::<libc::stat>::uninit();
    // SAFETY: fstat initializes stat on success.
    if unsafe { libc::fstat(descriptor_v1.as_raw_fd(), stat_v1.as_mut_ptr()) } != 0 {
        return Err(custody_error_v1(operation_v1));
    }
    // SAFETY: successful fstat initialized the value.
    Ok(unsafe { stat_v1.assume_init() })
}

fn cleanup_launch_image_v1(
    directory_path_v1: &CStr,
    directory_v1: BorrowedFd<'_>,
    file_v1: Option<BorrowedFd<'_>>,
    file_created_v1: bool,
) {
    // BEST-EFFORT: clear both flags even if a mutation set the forbidden file
    // flag after validation; cleanup must not retain an unexecutable image.
    // SAFETY: flags are cleared only on launcher-owned descriptors.
    unsafe {
        libc::fchflags(directory_v1.as_raw_fd(), 0);
        if file_created_v1 {
            if let Some(file_v1) = file_v1 {
                libc::fchflags(file_v1.as_raw_fd(), 0);
            }
            libc::unlinkat(directory_v1.as_raw_fd(), LAUNCH_FILE_NAME_V1.as_ptr().cast(), 0);
        }
        libc::rmdir(directory_path_v1.as_ptr());
    }
}

fn launch_error_v1(operation_v1: &'static str) -> ExplicitFdProcessErrorV1 {
    ExplicitFdProcessErrorV1::DarwinCustody {
        operation_v1,
        errno_v1: libc::EIO,
    }
}

#[cfg(test)]
mod tests {
    use super::super::{DarwinCustodyKindV1, DarwinImmutableCustodyV1};
    use super::DarwinEphemeralLaunchImageV1;
    use crate::ExplicitFdProcessErrorV1;
    use sha2::{Digest, Sha256};
    use std::fs::File;
    use std::os::fd::{AsFd, AsRawFd};

    #[test]
    fn immutable_source_and_immutable_launch_ancestry_are_not_conflated_v1() {
        let source_v1 = File::open("/bin/cat").expect("open executable fixture");
        let bytes_v1 = std::fs::read("/bin/cat").expect("read executable fixture");
        let digest_v1 = format!("{:x}", Sha256::digest(&bytes_v1));
        let custody_v1 = DarwinImmutableCustodyV1::try_copy_v1(
            source_v1.as_fd(),
            u64::try_from(bytes_v1.len()).expect("fixture size"),
            DarwinCustodyKindV1::Executable,
            &digest_v1,
        )
        .expect("build immutable source custody");
        let directory_mutation_v1 = DarwinEphemeralLaunchImageV1::try_copy_v1(
            custody_v1.descriptor_v1(),
            custody_v1.size_v1(),
            &digest_v1,
        )
        .expect("build ephemeral launch image");

        // Mutation: applying the source-custody immutable flag anywhere in the
        // launch ancestry must fail closed before Darwin dyld sees it.
        assert_eq!(
            // SAFETY: the flag is applied only to the test-owned directory and
            // its launch guard clears it during cleanup.
            unsafe {
                libc::fchflags(directory_mutation_v1.directory_v1.as_raw_fd(), libc::UF_IMMUTABLE)
            },
            0
        );
        assert_eq!(
            directory_mutation_v1.verify_before_spawn_v1(),
            Err(ExplicitFdProcessErrorV1::ExecutableIdentityDrift)
        );

        let file_mutation_v1 = DarwinEphemeralLaunchImageV1::try_copy_v1(
            custody_v1.descriptor_v1(),
            custody_v1.size_v1(),
            &digest_v1,
        )
        .expect("build second ephemeral launch image");
        assert_eq!(
            // SAFETY: the flag is applied only to the second test-owned image
            // and its launch guard clears it during cleanup.
            unsafe { libc::fchflags(file_mutation_v1.file_v1.as_raw_fd(), libc::UF_IMMUTABLE) },
            0
        );
        assert_eq!(
            file_mutation_v1.verify_before_spawn_v1(),
            Err(ExplicitFdProcessErrorV1::ExecutableIdentityDrift)
        );
    }
}
