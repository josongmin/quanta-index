use super::{descriptor_error_v1, descriptor_identity_v1, DescriptorIdentityV1};
use crate::ExplicitFdProcessErrorV1;
use sha2::{Digest, Sha256};
use std::ffi::c_char;
use std::os::fd::{AsFd, AsRawFd, BorrowedFd, FromRawFd, OwnedFd};

const COPY_BUFFER_BYTES_V1: usize = 64 * 1024;
const COPY_BUFFER_BYTES_U64_V1: u64 = 64 * 1024;

#[derive(Debug)]
pub(super) struct SealedMemfdV1 {
    pub(super) descriptor_v1: OwnedFd,
    identity_v1: DescriptorIdentityV1,
}

impl SealedMemfdV1 {
    fn try_copy_v1(
        source_v1: BorrowedFd<'_>,
        size_v1: u64,
        mode_v1: libc::mode_t,
        expected_sha256_v1: &str,
    ) -> Result<Self, ExplicitFdProcessErrorV1> {
        let descriptor_v1 = create_sealable_memfd_v1()?;
        copy_exact_bytes_v1(source_v1, descriptor_v1.as_fd(), size_v1)?;
        finish_and_seal_v1(descriptor_v1.as_fd(), mode_v1)?;
        verify_custody_sha256_v1(descriptor_v1.as_fd(), size_v1, expected_sha256_v1)?;
        let identity_v1 = descriptor_identity_v1(descriptor_v1.as_fd())?;
        if identity_v1.size_v1 != size_v1 || identity_v1.mode_v1 & 0o777 != mode_v1 {
            return Err(ExplicitFdProcessErrorV1::ExecutableIdentityDrift);
        }
        Ok(Self {
            descriptor_v1,
            identity_v1,
        })
    }

    fn verify_v1(&self) -> Result<(), ExplicitFdProcessErrorV1> {
        if descriptor_identity_v1(self.descriptor_v1.as_fd())? != self.identity_v1 {
            return Err(ExplicitFdProcessErrorV1::ExecutableIdentityDrift);
        }
        verify_seals_v1(self.descriptor_v1.as_fd(), required_seals_v1())
    }
}

fn create_sealable_memfd_v1() -> Result<OwnedFd, ExplicitFdProcessErrorV1> {
    static MEMFD_NAME_V1: &[u8] = b"quanta-explicit-fd\0";
    // SAFETY: memfd_create receives a static NUL-terminated name and
    // atomically creates a CLOEXEC descriptor with sealing enabled.
    let raw_v1 = unsafe {
        libc::syscall(
            libc::SYS_memfd_create,
            MEMFD_NAME_V1.as_ptr().cast::<c_char>(),
            libc::MFD_CLOEXEC | libc::MFD_ALLOW_SEALING,
        )
    };
    if raw_v1 < 0 {
        return Err(descriptor_error_v1("memfd_create_v1"));
    }
    let raw_v1 =
        i32::try_from(raw_v1).map_err(|_| descriptor_error_v1("memfd_descriptor_range_v1"))?;
    // SAFETY: memfd_create returned a fresh descriptor owned here.
    Ok(unsafe { OwnedFd::from_raw_fd(raw_v1) })
}

fn finish_and_seal_v1(
    descriptor_v1: BorrowedFd<'_>,
    mode_v1: libc::mode_t,
) -> Result<(), ExplicitFdProcessErrorV1> {
    // SAFETY: fchmod mutates only the private custody descriptor.
    if unsafe { libc::fchmod(descriptor_v1.as_raw_fd(), mode_v1) } != 0 {
        return Err(descriptor_error_v1("fchmod_memfd_v1"));
    }
    // SAFETY: fsync flushes the completed private custody contents/mode.
    if unsafe { libc::fsync(descriptor_v1.as_raw_fd()) } != 0 {
        return Err(descriptor_error_v1("fsync_memfd_v1"));
    }
    let required_seals_v1 = required_seals_v1();
    // SAFETY: F_ADD_SEALS applies only to the private MFD_ALLOW_SEALING fd.
    if unsafe { libc::fcntl(descriptor_v1.as_raw_fd(), libc::F_ADD_SEALS, required_seals_v1) } != 0
    {
        return Err(descriptor_error_v1("fcntl_add_seals_v1"));
    }
    verify_seals_v1(descriptor_v1, required_seals_v1)
}

const fn required_seals_v1() -> i32 {
    libc::F_SEAL_WRITE | libc::F_SEAL_GROW | libc::F_SEAL_SHRINK | libc::F_SEAL_SEAL
}

#[derive(Debug)]
pub(crate) struct PinnedExecutablePlatformV1 {
    pub(super) custody_v1: SealedMemfdV1,
}

impl PinnedExecutablePlatformV1 {
    pub(crate) fn try_new_v1(
        descriptor_v1: BorrowedFd<'_>,
        identity_v1: &DescriptorIdentityV1,
        expected_sha256_v1: &str,
    ) -> Result<Self, ExplicitFdProcessErrorV1> {
        Ok(Self {
            custody_v1: SealedMemfdV1::try_copy_v1(
                descriptor_v1,
                identity_v1.size_v1,
                0o500,
                expected_sha256_v1,
            )?,
        })
    }

    pub(crate) fn matches_loaded_identity_v1(&self, observed_v1: &DescriptorIdentityV1) -> bool {
        observed_v1 == &self.custody_v1.identity_v1
    }

    pub(crate) fn verify_before_spawn_v1(&self) -> Result<(), ExplicitFdProcessErrorV1> {
        self.custody_v1.verify_v1()
    }
}

#[derive(Debug)]
pub(crate) struct PinnedReadOnlyFilePlatformV1 {
    custody_v1: SealedMemfdV1,
}

impl PinnedReadOnlyFilePlatformV1 {
    pub(crate) fn try_new_v1(
        descriptor_v1: BorrowedFd<'_>,
        identity_v1: &DescriptorIdentityV1,
        expected_sha256_v1: &str,
    ) -> Result<Self, ExplicitFdProcessErrorV1> {
        Ok(Self {
            custody_v1: SealedMemfdV1::try_copy_v1(
                descriptor_v1,
                identity_v1.size_v1,
                0o400,
                expected_sha256_v1,
            )?,
        })
    }

    pub(crate) fn descriptor_v1(&self) -> BorrowedFd<'_> {
        self.custody_v1.descriptor_v1.as_fd()
    }

    pub(crate) fn try_open_read_only_for_spawn_v1(
        &self,
    ) -> Result<OwnedFd, ExplicitFdProcessErrorV1> {
        self.custody_v1.verify_v1()?;
        let path_v1 = std::ffi::CString::new(format!(
            "/proc/self/fd/{}",
            self.custody_v1.descriptor_v1.as_raw_fd()
        ))
        .expect("private descriptor path cannot contain NUL");
        // SAFETY: the live private sealed fd owns this kernel magic-link target.
        // Opening it creates a read-only OFD with an independent zero cursor.
        let raw_v1 = unsafe { libc::open(path_v1.as_ptr(), libc::O_RDONLY | libc::O_CLOEXEC) };
        if raw_v1 < 0 {
            return Err(descriptor_error_v1("open_read_only_custody_for_spawn_v1"));
        }
        // SAFETY: open returned a fresh descriptor owned here.
        let opened_v1 = unsafe { OwnedFd::from_raw_fd(raw_v1) };
        if descriptor_identity_v1(opened_v1.as_fd())? != self.custody_v1.identity_v1 {
            return Err(ExplicitFdProcessErrorV1::ExecutableIdentityDrift);
        }
        verify_seals_v1(opened_v1.as_fd(), required_seals_v1())?;
        Ok(opened_v1)
    }

    pub(crate) fn verify_before_spawn_v1(&self) -> Result<(), ExplicitFdProcessErrorV1> {
        self.custody_v1.verify_v1()
    }
}

fn copy_exact_bytes_v1(
    source_v1: BorrowedFd<'_>,
    custody_v1: BorrowedFd<'_>,
    size_v1: u64,
) -> Result<(), ExplicitFdProcessErrorV1> {
    let mut offset_v1 = 0_u64;
    let mut buffer_v1 = [0_u8; COPY_BUFFER_BYTES_V1];
    while offset_v1 < size_v1 {
        let remaining_v1 = usize::try_from((size_v1 - offset_v1).min(COPY_BUFFER_BYTES_U64_V1))
            .map_err(|_| ExplicitFdProcessErrorV1::InvalidExecutableDescriptor)?;
        // SAFETY: pread is offset-independent and writes into the live buffer.
        let read_v1 = unsafe {
            libc::pread(
                source_v1.as_raw_fd(),
                buffer_v1.as_mut_ptr().cast(),
                remaining_v1,
                i64::try_from(offset_v1)
                    .map_err(|_| ExplicitFdProcessErrorV1::InvalidExecutableDescriptor)?,
            )
        };
        if read_v1 < 0 {
            return Err(descriptor_error_v1("pread_source_v1"));
        }
        if read_v1 == 0 {
            return Err(ExplicitFdProcessErrorV1::ExecutableIdentityDrift);
        }
        let read_v1 = usize::try_from(read_v1)
            .map_err(|_| ExplicitFdProcessErrorV1::ExecutableIdentityDrift)?;
        write_all_descriptor_v1(custody_v1, &buffer_v1[..read_v1])?;
        offset_v1 = offset_v1
            .checked_add(
                u64::try_from(read_v1)
                    .map_err(|_| ExplicitFdProcessErrorV1::InvalidExecutableDescriptor)?,
            )
            .ok_or(ExplicitFdProcessErrorV1::InvalidExecutableDescriptor)?;
    }
    verify_no_trailing_bytes_v1(source_v1, size_v1)
}

fn verify_no_trailing_bytes_v1(
    source_v1: BorrowedFd<'_>,
    size_v1: u64,
) -> Result<(), ExplicitFdProcessErrorV1> {
    let mut trailing_v1 = [0_u8; 1];
    // SAFETY: one-byte trailing probe is bounded and offset-independent.
    let trailing_read_v1 = unsafe {
        libc::pread(
            source_v1.as_raw_fd(),
            trailing_v1.as_mut_ptr().cast(),
            1,
            i64::try_from(size_v1)
                .map_err(|_| ExplicitFdProcessErrorV1::InvalidExecutableDescriptor)?,
        )
    };
    if trailing_read_v1 < 0 {
        return Err(descriptor_error_v1("pread_source_trailing_v1"));
    }
    if trailing_read_v1 > 0 {
        return Err(ExplicitFdProcessErrorV1::ExecutableIdentityDrift);
    }
    Ok(())
}

fn write_all_descriptor_v1(
    descriptor_v1: BorrowedFd<'_>,
    mut bytes_v1: &[u8],
) -> Result<(), ExplicitFdProcessErrorV1> {
    while !bytes_v1.is_empty() {
        // SAFETY: bytes points to initialized memory for the requested length.
        let written_v1 = unsafe {
            libc::write(descriptor_v1.as_raw_fd(), bytes_v1.as_ptr().cast(), bytes_v1.len())
        };
        if written_v1 < 0 {
            if super::errno_v1() == libc::EINTR {
                continue;
            }
            return Err(descriptor_error_v1("write_custody_v1"));
        }
        let written_v1 = usize::try_from(written_v1)
            .map_err(|_| descriptor_error_v1("write_custody_range_v1"))?;
        if written_v1 == 0 {
            return Err(ExplicitFdProcessErrorV1::DescriptorOperation {
                operation_v1: "write_custody_zero_v1",
                errno_v1: libc::EIO,
            });
        }
        bytes_v1 = &bytes_v1[written_v1..];
    }
    Ok(())
}

fn verify_custody_sha256_v1(
    descriptor_v1: BorrowedFd<'_>,
    size_v1: u64,
    expected_sha256_v1: &str,
) -> Result<(), ExplicitFdProcessErrorV1> {
    let mut hasher_v1 = Sha256::new();
    let mut offset_v1 = 0_u64;
    let mut buffer_v1 = [0_u8; COPY_BUFFER_BYTES_V1];
    while offset_v1 < size_v1 {
        let remaining_v1 = usize::try_from((size_v1 - offset_v1).min(COPY_BUFFER_BYTES_U64_V1))
            .map_err(|_| ExplicitFdProcessErrorV1::InvalidExecutableDescriptor)?;
        // SAFETY: pread writes into the bounded initialized stack buffer.
        let read_v1 = unsafe {
            libc::pread(
                descriptor_v1.as_raw_fd(),
                buffer_v1.as_mut_ptr().cast(),
                remaining_v1,
                i64::try_from(offset_v1)
                    .map_err(|_| ExplicitFdProcessErrorV1::InvalidExecutableDescriptor)?,
            )
        };
        if read_v1 <= 0 {
            return Err(descriptor_error_v1("pread_custody_digest_v1"));
        }
        let read_v1 = usize::try_from(read_v1)
            .map_err(|_| ExplicitFdProcessErrorV1::CustodyDigestMismatch)?;
        hasher_v1.update(&buffer_v1[..read_v1]);
        offset_v1 = offset_v1
            .checked_add(
                u64::try_from(read_v1)
                    .map_err(|_| ExplicitFdProcessErrorV1::CustodyDigestMismatch)?,
            )
            .ok_or(ExplicitFdProcessErrorV1::CustodyDigestMismatch)?;
    }
    let actual_sha256_v1: [u8; 32] = hasher_v1.finalize().into();
    if !crate::pin::sha256_digest_matches_hex_v1(&actual_sha256_v1, expected_sha256_v1) {
        return Err(ExplicitFdProcessErrorV1::CustodyDigestMismatch);
    }
    Ok(())
}

fn verify_seals_v1(
    descriptor_v1: BorrowedFd<'_>,
    required_v1: i32,
) -> Result<(), ExplicitFdProcessErrorV1> {
    // SAFETY: F_GET_SEALS reads the immutable memfd seal mask.
    let seals_v1 = unsafe { libc::fcntl(descriptor_v1.as_raw_fd(), libc::F_GET_SEALS) };
    if seals_v1 < 0 {
        return Err(descriptor_error_v1("fcntl_get_seals_v1"));
    }
    if seals_v1 & required_v1 != required_v1 {
        return Err(ExplicitFdProcessErrorV1::ExecutableIdentityDrift);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use std::os::unix::net::UnixStream;

    #[test]
    fn trailing_source_probe_distinguishes_eof_drift_and_read_failure_v1() {
        let mut source_v1 = tempfile::tempfile().expect("create source fixture");
        source_v1.write_all(b"ab").expect("write source fixture");
        assert_eq!(verify_no_trailing_bytes_v1(source_v1.as_fd(), 2), Ok(()));
        assert_eq!(
            verify_no_trailing_bytes_v1(source_v1.as_fd(), 1),
            Err(ExplicitFdProcessErrorV1::ExecutableIdentityDrift)
        );

        let (unseekable_v1, _peer_v1) = UnixStream::pair().expect("create unseekable source");
        assert_eq!(
            verify_no_trailing_bytes_v1(unseekable_v1.as_fd(), 0),
            Err(ExplicitFdProcessErrorV1::DescriptorOperation {
                operation_v1: "pread_source_trailing_v1",
                errno_v1: libc::ESPIPE,
            })
        );
    }
}
