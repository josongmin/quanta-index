//! Same-socket process and executable observations, never PID-derived authority.
use crate::{
    IpcError, KernelPeerCredentials, MemoryLease, MemoryScope, PeerCredentials,
    PeerCredentialsSource,
};
#[cfg(target_os = "linux")]
use std::os::fd::{AsFd, OwnedFd};
use std::{
    os::{fd::BorrowedFd, unix::net::UnixStream},
    sync::Arc,
};

/// A local process/image-binding refusal observed before request bytes.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PeerBindingRefusalV1 {
    Unsupported,
    UnsupportedRoute,
    ProcessExited,
    ForeignProcess,
    InvalidKernelCredentials,
    Kernel {
        operation: &'static str,
        errno: i32,
    },
    #[cfg(target_os = "linux")]
    Image(quanta_explicit_fd_process::ExplicitFdProcessErrorV1),
}
impl std::fmt::Display for PeerBindingRefusalV1 {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Unsupported => f.write_str("connected peer process capture is unsupported"),
            Self::UnsupportedRoute => f.write_str("request is unsupported by this peer binding mode"),
            Self::ProcessExited => f.write_str("admitted peer process has exited"),
            Self::ForeignProcess => f.write_str("connected peer differs from the admitted process"),
            Self::InvalidKernelCredentials => {
                f.write_str("kernel peer process credentials are invalid")
            }
            Self::Kernel { operation, errno } => {
                write!(f, "peer process {operation} failed with errno {errno}")
            }
            #[cfg(target_os = "linux")]
            Self::Image(cause) => std::fmt::Display::fmt(cause, f),
        }
    }
}
impl std::error::Error for PeerBindingRefusalV1 {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            #[cfg(target_os = "linux")]
            Self::Image(cause) => Some(cause),
            _ => None,
        }
    }
}

/// Only the actual connected transport constructs this owner. Its observations
/// grant no installed-image, root, Source, EX or publication permission.
#[derive(Debug)]
pub struct KernelConnectedPeerProcessV1 {
    lease_v1: KernelConnectedPeerProcessLeaseV1,
}
#[derive(Debug)]
struct KernelPeerProcessDataV1 {
    #[cfg(target_os = "linux")]
    process_v1: OwnedFd,
    #[cfg(target_os = "linux")]
    image_v1: quanta_explicit_fd_process::ObservedExecutableImageV1,
    credentials_v1: PeerCredentials,
}
/// Share the exact descriptors and their existing Original grant, not new FDs.
#[derive(Clone, Debug)]
pub struct KernelConnectedPeerProcessLeaseV1 {
    // Descriptors and Arc payload retire before the final issuing grant.
    data_v1: Arc<KernelPeerProcessDataV1>,
    _memory_v1: MemoryLease,
}

#[cfg(target_os = "linux")]
fn ensure_process_live_v1(process: BorrowedFd<'_>) -> Result<(), PeerBindingRefusalV1> {
    let mut observed = [rustix::event::PollFd::from_borrowed_fd(
        process,
        rustix::event::PollFlags::IN,
    )];
    let ready = rustix::event::poll(
        &mut observed,
        Some(&rustix::event::Timespec {
            tv_sec: 0,
            tv_nsec: 0,
        }),
    )
    .map_err(|cause| PeerBindingRefusalV1::Kernel {
        operation: "poll",
        errno: cause.raw_os_error(),
    })?;
    if ready != 0 || !observed[0].revents().is_empty() {
        return Err(PeerBindingRefusalV1::ProcessExited);
    }
    Ok(())
}
#[cfg(target_os = "linux")]
fn open_live_process_image_v1(
    process: BorrowedFd<'_>,
    credentials: &PeerCredentials,
) -> Result<OwnedFd, PeerBindingRefusalV1> {
    use std::io::Write;
    ensure_process_live_v1(process)?;
    let pid = credentials
        .pid
        .filter(|pid| *pid != 0)
        .ok_or(PeerBindingRefusalV1::InvalidKernelCredentials)?;
    let mut path = [0_u8; 48];
    let len = {
        let mut cursor = std::io::Cursor::new(path.as_mut_slice());
        write!(&mut cursor, "/proc/{pid}/exe")
            .map_err(|_| PeerBindingRefusalV1::InvalidKernelCredentials)?;
        usize::try_from(cursor.position())
            .map_err(|_| PeerBindingRefusalV1::InvalidKernelCredentials)?
    };
    let path = std::str::from_utf8(&path[..len])
        .map_err(|_| PeerBindingRefusalV1::InvalidKernelCredentials)?;
    let image = rustix::fs::open(
        path,
        rustix::fs::OFlags::RDONLY | rustix::fs::OFlags::CLOEXEC,
        rustix::fs::Mode::empty(),
    )
    .map_err(|cause| PeerBindingRefusalV1::Kernel {
        operation: "open executable",
        errno: cause.raw_os_error(),
    })?;
    ensure_process_live_v1(process)?;
    Ok(image)
}
impl KernelConnectedPeerProcessLeaseV1 {
    /// Immutable aliases of one actual socket capture, independent of numeric
    /// credentials. This does not compare MemoryScope or Original authority.
    #[must_use]
    pub fn same_capture_v1(&self, other_v1: &Self) -> bool {
        Arc::ptr_eq(&self.data_v1, &other_v1.data_v1)
    }

    pub fn ensure_live_v1(&self) -> Result<(), PeerBindingRefusalV1> {
        #[cfg(target_os = "linux")]
        {
            ensure_process_live_v1(self.data_v1.process_v1.as_fd())
        }
        #[cfg(not(target_os = "linux"))]
        {
            Err(PeerBindingRefusalV1::Unsupported)
        }
    }
    #[cfg(target_os = "linux")]
    fn validate_current_image_v1(&self) -> Result<(), PeerBindingRefusalV1> {
        let live =
            open_live_process_image_v1(self.data_v1.process_v1.as_fd(), self.credentials_v1())?;
        self.data_v1
            .image_v1
            .verify_live_descriptor_v1(live.as_fd())
            .map_err(PeerBindingRefusalV1::Image)?;
        self.ensure_live_v1()
    }
    /// Installed policy sees the retained capture image, never another image
    /// observed later. Successful policy checks revalidate the current process
    /// image before returning. This does not atomically prevent later exec.
    pub fn with_live_executable_v1<T, E>(
        &self,
        check: impl for<'fd> FnOnce(BorrowedFd<'fd>) -> Result<T, E>,
    ) -> Result<Result<T, E>, PeerBindingRefusalV1> {
        #[cfg(target_os = "linux")]
        {
            self.validate_current_image_v1()?;
            let result = check(self.data_v1.image_v1.as_fd());
            if result.is_ok() {
                self.validate_current_image_v1()?;
            }
            Ok(result)
        }
        #[cfg(not(target_os = "linux"))]
        {
            let _unused_check = check;
            Err(PeerBindingRefusalV1::Unsupported)
        }
    }
    /// The actual connection is checked immediately before the canonical
    /// transport writes. Root/cwd are not inferred from process metadata.
    pub(crate) fn validate_connected_stream_v1(
        &self,
        stream: &UnixStream,
    ) -> Result<(), PeerBindingRefusalV1> {
        self.ensure_live_v1()?;
        let observed =
            KernelPeerCredentials
                .peer_credentials(stream)
                .map_err(|cause| match cause.raw_os_error() {
                    Some(errno) => PeerBindingRefusalV1::Kernel {
                        operation: "socket credentials",
                        errno,
                    },
                    None => PeerBindingRefusalV1::InvalidKernelCredentials,
                })?;
        if &observed != self.credentials_v1() {
            return Err(PeerBindingRefusalV1::ForeignProcess);
        }
        #[cfg(target_os = "linux")]
        self.validate_current_image_v1()?;
        self.ensure_live_v1()
    }
    #[must_use]
    pub fn credentials_v1(&self) -> &PeerCredentials {
        &self.data_v1.credentials_v1
    }
    pub fn pidfd_v1(&self) -> Result<BorrowedFd<'_>, IpcError> {
        #[cfg(target_os = "linux")]
        {
            Ok(self.data_v1.process_v1.as_fd())
        }
        #[cfg(not(target_os = "linux"))]
        {
            Err(unsupported_v1())
        }
    }
}
impl KernelConnectedPeerProcessV1 {
    #[must_use]
    pub fn credentials_v1(&self) -> &PeerCredentials {
        self.lease_v1.credentials_v1()
    }
    pub fn pidfd_v1(&self) -> Result<BorrowedFd<'_>, IpcError> {
        self.lease_v1.pidfd_v1()
    }
    #[must_use]
    pub fn share_lease_v1(&self) -> KernelConnectedPeerProcessLeaseV1 {
        self.lease_v1.clone()
    }
    pub(crate) fn try_capture_v1(
        stream_v1: &UnixStream,
        observed_v1: PeerCredentials,
        memory_v1: &MemoryScope,
    ) -> Result<Self, IpcError> {
        let actual_v1 = KernelPeerCredentials
            .peer_credentials(stream_v1)
            .map_err(IpcError::Io)?;
        if actual_v1 != observed_v1 {
            return Err(IpcError::Io(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "connected peer credentials changed during process capture",
            )));
        }
        if !matches!(actual_v1.pid, Some(pid) if pid != 0) {
            return Err(unsupported_v1());
        }
        Self::capture_platform_v1(stream_v1, actual_v1, memory_v1)
    }
    #[cfg(target_os = "linux")]
    fn capture_platform_v1(
        stream_v1: &UnixStream,
        credentials_v1: PeerCredentials,
        memory_v1: &MemoryScope,
    ) -> Result<Self, IpcError> {
        use quanta_index_core::index::MemoryLifetime;
        let layout_v1 = MemoryLease::shared_allocation_backing_layout(std::alloc::Layout::new::<
            KernelPeerProcessDataV1,
        >())
        .map_err(|cause| crate::codec::classify_client_memory_error(0, cause))?;
        let bytes_v1 = u64::try_from(layout_v1.size()).map_err(|_| unsupported_v1())?;
        // Admit the complete enlarged Arc before any captured FD or Arc birth.
        let grant_v1 = memory_v1
            .reserve(bytes_v1, MemoryLifetime::Retained)
            .map_err(|cause| crate::codec::classify_client_memory_error(bytes_v1, cause))?;
        let process_v1 =
            nix::sys::socket::getsockopt(stream_v1, nix::sys::socket::sockopt::PeerPidfd).map_err(
                |cause| {
                    if matches!(
                        cause,
                        nix::errno::Errno::ENOPROTOOPT
                            | nix::errno::Errno::ENOSYS
                            | nix::errno::Errno::EOPNOTSUPP
                    ) {
                        IpcError::Io(std::io::Error::new(std::io::ErrorKind::Unsupported, cause))
                    } else {
                        IpcError::Io(std::io::Error::from(cause))
                    }
                },
            )?;
        let flags =
            rustix::io::fcntl_getfd(&process_v1).map_err(|cause| IpcError::Io(cause.into()))?;
        if !flags.contains(rustix::io::FdFlags::CLOEXEC) {
            return Err(IpcError::Io(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "connected peer process descriptor is not close-on-exec",
            )));
        }
        let image_fd_v1 = open_live_process_image_v1(process_v1.as_fd(), &credentials_v1)
            .map_err(IpcError::PeerBindingRefused)?;
        let image_v1 =
            quanta_explicit_fd_process::ObservedExecutableImageV1::try_from_owned_descriptor_v1(
                image_fd_v1,
            )
            .map_err(PeerBindingRefusalV1::Image)
            .map_err(IpcError::PeerBindingRefused)?;
        ensure_process_live_v1(process_v1.as_fd()).map_err(IpcError::PeerBindingRefused)?;
        Ok(Self {
            lease_v1: KernelConnectedPeerProcessLeaseV1 {
                data_v1: Arc::new(KernelPeerProcessDataV1 {
                    process_v1,
                    image_v1,
                    credentials_v1,
                }),
                _memory_v1: grant_v1,
            },
        })
    }
    #[cfg(not(target_os = "linux"))]
    fn capture_platform_v1(
        _stream: &UnixStream,
        _credentials: PeerCredentials,
        _memory: &MemoryScope,
    ) -> Result<Self, IpcError> {
        Err(unsupported_v1())
    }
}
fn unsupported_v1() -> IpcError {
    IpcError::Io(std::io::Error::new(
        std::io::ErrorKind::Unsupported,
        "connected kernel peer process instance is unavailable",
    ))
}
