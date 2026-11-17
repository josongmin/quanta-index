use crate::{platform, ExplicitFdProcessErrorV1};
use std::fs::File;
use std::mem::MaybeUninit;
use std::os::fd::{AsFd, AsRawFd, BorrowedFd, RawFd};
use std::os::unix::net::UnixDatagram;

/// Safely duplicate an explicit child pipe mapping into an owned CLOEXEC file.
///
/// This is a child-entry boundary, not a generic raw-FD ownership conversion.
/// The inherited descriptor remains process-owned and is never wrapped or
/// closed here; the returned file owns a fresh `F_DUPFD_CLOEXEC` duplicate.
/// Requiring the inherited descriptor itself to be non-CLOEXEC distinguishes
/// the explicit child mapping produced by this crate from parent sources.
pub fn try_duplicate_inherited_read_pipe_v1(
    raw_fd_v1: RawFd,
) -> Result<File, ExplicitFdProcessErrorV1> {
    if raw_fd_v1 < 3 {
        return Err(ExplicitFdProcessErrorV1::InvalidInheritedDescriptor);
    }
    Ok(File::from(platform::duplicate_inherited_read_pipe_v1(raw_fd_v1)?))
}

/// Safely duplicate an explicit child control socket into owned CLOEXEC custody.
///
/// This boundary accepts only a connected Unix datagram socket deliberately
/// inherited across exec. The inherited descriptor stays process-owned but is
/// immediately resealed `FD_CLOEXEC`; the returned socket owns a fresh
/// `F_DUPFD_CLOEXEC` duplicate. Stream sockets,
/// unconnected datagram sockets, non-Unix sockets, and ambient CLOEXEC
/// descriptors fail closed.
pub fn try_duplicate_inherited_unix_datagram_v1(
    raw_fd_v1: RawFd,
) -> Result<UnixDatagram, ExplicitFdProcessErrorV1> {
    if raw_fd_v1 < 3 {
        return Err(ExplicitFdProcessErrorV1::InvalidInheritedUnixDatagramDescriptor);
    }
    // SAFETY: the borrow lasts only through descriptor inspection and
    // duplication. This function never assumes ownership of or closes the
    // process-owned inherited descriptor.
    let inherited_v1 = unsafe { BorrowedFd::borrow_raw(raw_fd_v1) };
    if platform::descriptor_is_cloexec_v1(inherited_v1)
        .map_err(|_| ExplicitFdProcessErrorV1::InvalidInheritedUnixDatagramDescriptor)?
    {
        return Err(ExplicitFdProcessErrorV1::InvalidInheritedUnixDatagramDescriptor);
    }
    let duplicate_v1 = platform::duplicate_cloexec_v1(inherited_v1)
        .map_err(|_| ExplicitFdProcessErrorV1::InvalidInheritedUnixDatagramDescriptor)?;
    validate_connected_unix_datagram_v1(duplicate_v1.as_fd())?;
    platform::set_descriptor_cloexec_v1(inherited_v1)
        .map_err(|_| ExplicitFdProcessErrorV1::InvalidInheritedUnixDatagramDescriptor)?;
    Ok(UnixDatagram::from(duplicate_v1))
}

/// Duplicates one explicit inherited read-write regular file into owned
/// CLOEXEC custody and reseals the process-owned inherited descriptor.
pub fn try_duplicate_inherited_regular_file_v1(
    raw_fd_v1: RawFd,
) -> Result<File, ExplicitFdProcessErrorV1> {
    if raw_fd_v1 < 3 {
        return Err(ExplicitFdProcessErrorV1::InvalidInheritedRegularFileDescriptor);
    }
    // SAFETY: the live inherited descriptor is borrowed only for inspection
    // and duplication; this function never takes ownership of that raw fd.
    let inherited_v1 = unsafe { BorrowedFd::borrow_raw(raw_fd_v1) };
    if platform::descriptor_is_cloexec_v1(inherited_v1)
        .map_err(|_| ExplicitFdProcessErrorV1::InvalidInheritedRegularFileDescriptor)?
    {
        return Err(ExplicitFdProcessErrorV1::InvalidInheritedRegularFileDescriptor);
    }
    let duplicate_v1 = platform::duplicate_cloexec_v1(inherited_v1)
        .map_err(|_| ExplicitFdProcessErrorV1::InvalidInheritedRegularFileDescriptor)?;
    validate_retained_regular_file_v1(duplicate_v1.as_fd())
        .map_err(|_| ExplicitFdProcessErrorV1::InvalidInheritedRegularFileDescriptor)?;
    platform::set_descriptor_cloexec_v1(inherited_v1)
        .map_err(|_| ExplicitFdProcessErrorV1::InvalidInheritedRegularFileDescriptor)?;
    Ok(File::from(duplicate_v1))
}

/// Import an optional explicitly mapped regular file. Only `EBADF` means the
/// child mapping is absent; a present but malformed descriptor is a refusal.
pub fn try_duplicate_optional_inherited_regular_file_v1(
    raw_fd_v1: RawFd,
) -> Result<Option<File>, ExplicitFdProcessErrorV1> {
    if raw_fd_v1 < 3 {
        return Err(ExplicitFdProcessErrorV1::InvalidInheritedRegularFileDescriptor);
    }
    // SAFETY: F_GETFD only inspects the process-owned descriptor. The optional
    // probe never assumes ownership; the mandatory importer performs all
    // descriptor checks and creates the owned CLOEXEC duplicate.
    if unsafe { libc::fcntl(raw_fd_v1, libc::F_GETFD) } < 0 {
        return match std::io::Error::last_os_error().raw_os_error() {
            Some(libc::EBADF) => Ok(None),
            _ => Err(ExplicitFdProcessErrorV1::InvalidInheritedRegularFileDescriptor),
        };
    }
    try_duplicate_inherited_regular_file_v1(raw_fd_v1).map(Some)
}

/// Duplicates one explicit inherited read-only regular file into owned
/// CLOEXEC custody and reseals the process-owned inherited descriptor.
/// The parent maps it through `with_pinned_read_only_file_v1`.
pub fn try_duplicate_inherited_read_only_regular_file_v1(
    raw_fd_v1: RawFd,
) -> Result<File, ExplicitFdProcessErrorV1> {
    if raw_fd_v1 < 3 {
        return Err(ExplicitFdProcessErrorV1::InvalidInheritedReadOnlyRegularFileDescriptor);
    }
    // SAFETY: this borrows the live inherited descriptor only for inspection
    // and duplication; the original remains process-owned.
    let inherited_v1 = unsafe { BorrowedFd::borrow_raw(raw_fd_v1) };
    if platform::descriptor_is_cloexec_v1(inherited_v1)
        .map_err(|_| ExplicitFdProcessErrorV1::InvalidInheritedReadOnlyRegularFileDescriptor)?
    {
        return Err(ExplicitFdProcessErrorV1::InvalidInheritedReadOnlyRegularFileDescriptor);
    }
    let duplicate_v1 = platform::duplicate_cloexec_v1(inherited_v1)
        .map_err(|_| ExplicitFdProcessErrorV1::InvalidInheritedReadOnlyRegularFileDescriptor)?;
    if !platform::descriptor_identity_v1(duplicate_v1.as_fd())
        .map_err(|_| ExplicitFdProcessErrorV1::InvalidInheritedReadOnlyRegularFileDescriptor)?
        .regular_file_v1
        || !platform::descriptor_is_read_only_v1(duplicate_v1.as_fd())
            .map_err(|_| ExplicitFdProcessErrorV1::InvalidInheritedReadOnlyRegularFileDescriptor)?
    {
        return Err(ExplicitFdProcessErrorV1::InvalidInheritedReadOnlyRegularFileDescriptor);
    }
    platform::set_descriptor_cloexec_v1(inherited_v1)
        .map_err(|_| ExplicitFdProcessErrorV1::InvalidInheritedReadOnlyRegularFileDescriptor)?;
    Ok(File::from(duplicate_v1))
}

pub(crate) fn validate_retained_regular_file_v1(
    descriptor_v1: BorrowedFd<'_>,
) -> Result<(), ExplicitFdProcessErrorV1> {
    if !platform::descriptor_identity_v1(descriptor_v1)?.regular_file_v1 {
        return Err(ExplicitFdProcessErrorV1::InvalidRetainedRegularFileDescriptor);
    }
    // SAFETY: F_GETFL only observes the live borrowed descriptor.
    let flags_v1 = unsafe { libc::fcntl(descriptor_v1.as_raw_fd(), libc::F_GETFL) };
    if flags_v1 < 0 || flags_v1 & libc::O_ACCMODE != libc::O_RDWR {
        return Err(ExplicitFdProcessErrorV1::InvalidRetainedRegularFileDescriptor);
    }
    Ok(())
}

pub(crate) fn validate_connected_unix_datagram_v1(
    descriptor_v1: BorrowedFd<'_>,
) -> Result<(), ExplicitFdProcessErrorV1> {
    let mut socket_type_v1: libc::c_int = 0;
    let mut socket_type_len_v1 = std::mem::size_of::<libc::c_int>() as libc::socklen_t;
    // SAFETY: the value and length pointers name initialized writable storage.
    let socket_type_result_v1 = unsafe {
        libc::getsockopt(
            descriptor_v1.as_raw_fd(),
            libc::SOL_SOCKET,
            libc::SO_TYPE,
            std::ptr::addr_of_mut!(socket_type_v1).cast(),
            &mut socket_type_len_v1,
        )
    };
    if socket_type_result_v1 != 0
        || socket_type_len_v1 as usize != std::mem::size_of::<libc::c_int>()
        || socket_type_v1 != libc::SOCK_DGRAM
    {
        return Err(ExplicitFdProcessErrorV1::InvalidInheritedUnixDatagramDescriptor);
    }

    let mut peer_v1 = MaybeUninit::<libc::sockaddr_storage>::zeroed();
    let mut peer_len_v1 = std::mem::size_of::<libc::sockaddr_storage>() as libc::socklen_t;
    // SAFETY: getpeername writes at most peer_len_v1 bytes into peer_v1 and
    // initializes peer_len_v1 on success.
    let peer_result_v1 = unsafe {
        libc::getpeername(descriptor_v1.as_raw_fd(), peer_v1.as_mut_ptr().cast(), &mut peer_len_v1)
    };
    if peer_result_v1 != 0
        || peer_len_v1 < std::mem::size_of::<libc::sa_family_t>() as libc::socklen_t
    {
        return Err(ExplicitFdProcessErrorV1::InvalidInheritedUnixDatagramDescriptor);
    }
    // SAFETY: successful getpeername initialized at least the family field.
    let peer_v1 = unsafe { peer_v1.assume_init() };
    if i32::from(peer_v1.ss_family) != libc::AF_UNIX {
        return Err(ExplicitFdProcessErrorV1::InvalidInheritedUnixDatagramDescriptor);
    }
    Ok(())
}
