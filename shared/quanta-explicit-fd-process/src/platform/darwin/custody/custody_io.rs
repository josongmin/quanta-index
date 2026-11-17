use super::super::custody_error_v1;
use crate::ExplicitFdProcessErrorV1;
use sha2::{Digest, Sha256};
use std::os::fd::{AsRawFd, BorrowedFd};

const COPY_BUFFER_BYTES_V1: usize = 64 * 1024;
const COPY_BUFFER_BYTES_U64_V1: u64 = 64 * 1024;

pub(super) fn copy_exact_bytes_v1(
    source_v1: BorrowedFd<'_>,
    destination_v1: BorrowedFd<'_>,
    size_v1: u64,
) -> Result<(), ExplicitFdProcessErrorV1> {
    let mut offset_v1 = 0_u64;
    let mut buffer_v1 = [0_u8; COPY_BUFFER_BYTES_V1];
    while offset_v1 < size_v1 {
        let count_v1 = usize::try_from((size_v1 - offset_v1).min(COPY_BUFFER_BYTES_U64_V1))
            .map_err(|_| ExplicitFdProcessErrorV1::ExecutableIdentityDrift)?;
        // SAFETY: pread is offset-independent and writes into the live buffer.
        let read_v1 = unsafe {
            libc::pread(
                source_v1.as_raw_fd(),
                buffer_v1.as_mut_ptr().cast(),
                count_v1,
                i64::try_from(offset_v1)
                    .map_err(|_| ExplicitFdProcessErrorV1::ExecutableIdentityDrift)?,
            )
        };
        if read_v1 <= 0 {
            return Err(custody_error_v1("pread_custody_source_v1"));
        }
        let read_v1 = usize::try_from(read_v1)
            .map_err(|_| ExplicitFdProcessErrorV1::ExecutableIdentityDrift)?;
        write_all_v1(destination_v1, &buffer_v1[..read_v1])?;
        offset_v1 = offset_v1
            .checked_add(
                u64::try_from(read_v1)
                    .map_err(|_| ExplicitFdProcessErrorV1::ExecutableIdentityDrift)?,
            )
            .ok_or(ExplicitFdProcessErrorV1::ExecutableIdentityDrift)?;
    }
    let mut trailing_v1 = [0_u8; 1];
    // SAFETY: one-byte trailing probe is bounded and offset-independent.
    if unsafe {
        libc::pread(
            source_v1.as_raw_fd(),
            trailing_v1.as_mut_ptr().cast(),
            1,
            i64::try_from(size_v1)
                .map_err(|_| ExplicitFdProcessErrorV1::ExecutableIdentityDrift)?,
        )
    } != 0
    {
        return Err(ExplicitFdProcessErrorV1::ExecutableIdentityDrift);
    }
    Ok(())
}

fn write_all_v1(
    destination_v1: BorrowedFd<'_>,
    mut bytes_v1: &[u8],
) -> Result<(), ExplicitFdProcessErrorV1> {
    while !bytes_v1.is_empty() {
        // SAFETY: bytes is a live initialized slice.
        let written_v1 = unsafe {
            libc::write(destination_v1.as_raw_fd(), bytes_v1.as_ptr().cast(), bytes_v1.len())
        };
        if written_v1 < 0 {
            if super::super::errno_v1() == libc::EINTR {
                continue;
            }
            return Err(custody_error_v1("write_custody_v1"));
        }
        let written_v1 =
            usize::try_from(written_v1).map_err(|_| custody_error_v1("write_custody_range_v1"))?;
        if written_v1 == 0 {
            return Err(ExplicitFdProcessErrorV1::DarwinCustody {
                operation_v1: "write_custody_zero_v1",
                errno_v1: libc::EIO,
            });
        }
        bytes_v1 = &bytes_v1[written_v1..];
    }
    Ok(())
}

pub(super) fn verify_custody_sha256_v1(
    file_v1: BorrowedFd<'_>,
    size_v1: u64,
    expected_sha256_v1: &str,
) -> Result<(), ExplicitFdProcessErrorV1> {
    let mut offset_v1 = 0_u64;
    let mut hasher_v1 = Sha256::new();
    let mut buffer_v1 = [0_u8; COPY_BUFFER_BYTES_V1];
    while offset_v1 < size_v1 {
        let count_v1 = usize::try_from((size_v1 - offset_v1).min(COPY_BUFFER_BYTES_U64_V1))
            .map_err(|_| ExplicitFdProcessErrorV1::CustodyDigestMismatch)?;
        // SAFETY: pread writes into the bounded stack buffer.
        let read_v1 = unsafe {
            libc::pread(
                file_v1.as_raw_fd(),
                buffer_v1.as_mut_ptr().cast(),
                count_v1,
                i64::try_from(offset_v1)
                    .map_err(|_| ExplicitFdProcessErrorV1::CustodyDigestMismatch)?,
            )
        };
        if read_v1 <= 0 {
            return Err(custody_error_v1("pread_custody_digest_v1"));
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

pub(super) fn sync_descriptor_v1(
    descriptor_v1: BorrowedFd<'_>,
    operation_v1: &'static str,
) -> Result<(), ExplicitFdProcessErrorV1> {
    // SAFETY: fsync operates on the live custody descriptor.
    if unsafe { libc::fsync(descriptor_v1.as_raw_fd()) } != 0 {
        return Err(custody_error_v1(operation_v1));
    }
    Ok(())
}
