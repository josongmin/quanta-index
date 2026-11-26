use crate::platform;
use crate::{
    ExplicitFdProcessErrorV1, MAX_PINNED_EXECUTABLE_BYTES_V1, MAX_PINNED_READ_ONLY_FILE_BYTES_V1,
};
use std::fmt;
use std::os::fd::{AsFd, BorrowedFd, OwnedFd};
use std::sync::Arc;

fn executable_source_identity_v1(
    source_v1: BorrowedFd<'_>,
) -> Result<platform::DescriptorIdentityV1, ExplicitFdProcessErrorV1> {
    validate_cloexec_read_only_source_v1(source_v1)?;
    let identity_v1 = platform::descriptor_identity_v1(source_v1)?;
    if !identity_v1.regular_file_v1
        || identity_v1.size_v1 == 0
        || identity_v1.size_v1 > MAX_PINNED_EXECUTABLE_BYTES_V1
        || identity_v1.mode_v1 & 0o111 == 0
    {
        return Err(ExplicitFdProcessErrorV1::InvalidExecutableDescriptor);
    }
    Ok(identity_v1)
}

/// Kernel-opened executable observation, without installed-image permission.
/// The caller owns its admission and process binding. This object neither
/// copies/hashes bytes nor issues a process/Original/publication authority.
#[cfg(target_os = "linux")]
#[derive(Debug)]
pub struct ObservedExecutableImageV1 {
    descriptor_v1: OwnedFd,
    identity_v1: platform::DescriptorIdentityV1,
}
#[cfg(target_os = "linux")]
impl ObservedExecutableImageV1 {
    /// Take the already-admitted actual kernel descriptor without another dup.
    pub fn try_from_owned_descriptor_v1(
        descriptor_v1: OwnedFd,
    ) -> Result<Self, ExplicitFdProcessErrorV1> {
        let identity_v1 = executable_source_identity_v1(descriptor_v1.as_fd())?;
        Ok(Self {
            descriptor_v1,
            identity_v1,
        })
    }

    /// Both retained and live image objects must still match the first actual
    /// observation. A new image with equal bytes is a different observation.
    pub fn verify_live_descriptor_v1(
        &self,
        live_v1: BorrowedFd<'_>,
    ) -> Result<(), ExplicitFdProcessErrorV1> {
        let retained_v1 = executable_source_identity_v1(self.descriptor_v1.as_fd())?;
        let live_v1 = executable_source_identity_v1(live_v1)?;
        if retained_v1 != self.identity_v1 || live_v1 != self.identity_v1 {
            return Err(ExplicitFdProcessErrorV1::ExecutableIdentityDrift);
        }
        Ok(())
    }
}
#[cfg(target_os = "linux")]
impl AsFd for ObservedExecutableImageV1 {
    fn as_fd(&self) -> BorrowedFd<'_> {
        self.descriptor_v1.as_fd()
    }
}

#[derive(Clone)]
pub struct PinnedExecutableV1 {
    pub(crate) inner_v1: Arc<PinnedExecutableInnerV1>,
}

pub(crate) struct PinnedExecutableInnerV1 {
    pub(crate) platform_v1: platform::PinnedExecutablePlatformV1,
    // A normal exec and the canonical sealed launch have distinct inodes.
    // Both must originate from this one verified installation, never a path
    // or digest supplied by the peer being authenticated.
    #[cfg(target_os = "linux")]
    source_v1: OwnedFd,
    #[cfg(target_os = "linux")]
    source_identity_v1: platform::DescriptorIdentityV1,
}

impl fmt::Debug for PinnedExecutableV1 {
    fn fmt(&self, formatter_v1: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter_v1
            .debug_struct("PinnedExecutableV1")
            .finish_non_exhaustive()
    }
}

impl PinnedExecutableV1 {
    pub fn try_pin_v1(
        source_v1: impl AsFd,
        expected_sha256_v1: &str,
    ) -> Result<Self, ExplicitFdProcessErrorV1> {
        validate_expected_sha256_v1(expected_sha256_v1)?;
        let identity_v1 = executable_source_identity_v1(source_v1.as_fd())?;
        let source_v1 = platform::duplicate_cloexec_v1(source_v1.as_fd())?;
        if platform::descriptor_identity_v1(source_v1.as_fd())? != identity_v1 {
            return Err(ExplicitFdProcessErrorV1::ExecutableIdentityDrift);
        }
        let platform_v1 = platform::PinnedExecutablePlatformV1::try_new_v1(
            source_v1.as_fd(),
            &identity_v1,
            expected_sha256_v1,
        )?;
        if platform::descriptor_identity_v1(source_v1.as_fd())? != identity_v1 {
            return Err(ExplicitFdProcessErrorV1::ExecutableIdentityDrift);
        }
        Ok(Self {
            inner_v1: Arc::new(PinnedExecutableInnerV1 {
                platform_v1,
                #[cfg(target_os = "linux")]
                source_v1,
                #[cfg(target_os = "linux")]
                source_identity_v1: identity_v1,
            }),
        })
    }

    /// Check a kernel-opened executable against this exact installed image.
    /// This validates image-object custody only. The caller must bind the FD
    /// to the actual connected process and keep its process lease alive.
    /// No image bytes are copied or hashed at the request boundary.
    #[cfg(target_os = "linux")]
    pub fn verify_loaded_image_descriptor_v1(
        &self,
        loaded_v1: impl AsFd,
    ) -> Result<(), ExplicitFdProcessErrorV1> {
        self.verify_v1()?;
        validate_cloexec_read_only_source_v1(loaded_v1.as_fd())?;
        let observed_v1 = platform::descriptor_identity_v1(loaded_v1.as_fd())?;
        if self
            .inner_v1
            .platform_v1
            .matches_loaded_identity_v1(&observed_v1)
        {
            return Ok(());
        }
        let current_source_v1 = platform::descriptor_identity_v1(self.inner_v1.source_v1.as_fd())?;
        if current_source_v1 != self.inner_v1.source_identity_v1
            || observed_v1 != self.inner_v1.source_identity_v1
        {
            return Err(ExplicitFdProcessErrorV1::ExecutableIdentityDrift);
        }
        Ok(())
    }

    pub(crate) fn verify_v1(&self) -> Result<(), ExplicitFdProcessErrorV1> {
        self.inner_v1.platform_v1.verify_before_spawn_v1()
    }
}

#[derive(Clone, Debug)]
pub struct PinnedReadOnlyFileV1 {
    pub(crate) inner_v1: Arc<platform::PinnedReadOnlyFilePlatformV1>,
}

impl PinnedReadOnlyFileV1 {
    pub fn try_pin_v1(
        source_v1: impl AsFd,
        expected_sha256_v1: &str,
    ) -> Result<Self, ExplicitFdProcessErrorV1> {
        validate_expected_sha256_v1(expected_sha256_v1)?;
        validate_cloexec_read_only_source_v1(source_v1.as_fd())?;
        let identity_v1 = platform::descriptor_identity_v1(source_v1.as_fd())?;
        if !identity_v1.regular_file_v1
            || identity_v1.size_v1 == 0
            || identity_v1.size_v1 > MAX_PINNED_READ_ONLY_FILE_BYTES_V1
        {
            return Err(ExplicitFdProcessErrorV1::InvalidReadOnlyFileDescriptor);
        }
        Ok(Self {
            inner_v1: Arc::new(platform::PinnedReadOnlyFilePlatformV1::try_new_v1(
                source_v1.as_fd(),
                &identity_v1,
                expected_sha256_v1,
            )?),
        })
    }
}

#[derive(Clone, Debug)]
pub struct PinnedWorkingDirectoryV1 {
    pub(crate) inner_v1: Arc<PinnedWorkingDirectoryInnerV1>,
}

#[derive(Debug)]
pub(crate) struct PinnedWorkingDirectoryInnerV1 {
    pub(crate) descriptor_v1: OwnedFd,
    identity_v1: platform::DescriptorIdentityV1,
}

impl PinnedWorkingDirectoryV1 {
    /// Exact allocation of the owned descriptor/identity Arc. The caller must
    /// reserve this backing before `try_pin_v1` and retain that grant through
    /// the pinned directory's last use.
    #[expect(
        clippy::manual_ok_err,
        reason = "Result::ok is forbidden; an unrepresentable admission estimate remains None"
    )]
    pub fn native_backing_bytes_v1() -> Option<u64> {
        let layout_v1 = match std::alloc::Layout::new::<[usize; 2]>()
            .extend(std::alloc::Layout::new::<PinnedWorkingDirectoryInnerV1>())
        {
            Ok((layout_v1, _)) => layout_v1,
            Err(_) => return None,
        };
        match u64::try_from(layout_v1.pad_to_align().size()) {
            Ok(bytes_v1) => Some(bytes_v1),
            Err(_) => None,
        }
    }
    pub fn try_pin_v1(source_v1: impl AsFd) -> Result<Self, ExplicitFdProcessErrorV1> {
        validate_cloexec_read_only_source_v1(source_v1.as_fd())?;
        let descriptor_v1 = platform::duplicate_cloexec_v1(source_v1.as_fd())?;
        let identity_v1 = platform::descriptor_identity_v1(descriptor_v1.as_fd())?;
        if !identity_v1.directory_v1 {
            return Err(ExplicitFdProcessErrorV1::InvalidWorkingDirectoryDescriptor);
        }
        Ok(Self {
            inner_v1: Arc::new(PinnedWorkingDirectoryInnerV1 {
                descriptor_v1,
                identity_v1,
            }),
        })
    }

    /// Compare a separately opened directory with the pinned workspace object.
    /// Path spelling and mutable directory contents are not object identity.
    pub fn verify_same_directory_v1(
        &self,
        source_v1: impl AsFd,
    ) -> Result<(), ExplicitFdProcessErrorV1> {
        self.verify_v1()?;
        let observed_v1 = platform::descriptor_identity_v1(source_v1.as_fd())?;
        if !observed_v1.directory_v1
            || observed_v1.device_v1 != self.inner_v1.identity_v1.device_v1
            || observed_v1.inode_v1 != self.inner_v1.identity_v1.inode_v1
        {
            return Err(ExplicitFdProcessErrorV1::WorkingDirectoryIdentityDrift);
        }
        Ok(())
    }

    /// Compare authenticated observation DATA with the retained startup root.
    /// The numbers do not construct a directory owner or any Source authority.
    pub fn verify_observed_directory_identity_v1(
        &self,
        device_v1: u64,
        inode_v1: u64,
    ) -> Result<(), ExplicitFdProcessErrorV1> {
        self.verify_v1()?;
        if device_v1 != self.inner_v1.identity_v1.device_v1
            || inode_v1 != self.inner_v1.identity_v1.inode_v1
        {
            return Err(ExplicitFdProcessErrorV1::WorkingDirectoryIdentityDrift);
        }
        Ok(())
    }

    pub(crate) fn verify_v1(&self) -> Result<(), ExplicitFdProcessErrorV1> {
        let observed_v1 = platform::descriptor_identity_v1(self.inner_v1.descriptor_v1.as_fd())?;
        // A working directory is expected to gain and lose entries while a
        // pinned session is alive. Directory size and timestamps therefore
        // are mutable content metadata, not descriptor identity. The owned FD
        // itself cannot be path-replaced; bind only the stable object/type and
        // permission identity here.
        if observed_v1.device_v1 != self.inner_v1.identity_v1.device_v1
            || observed_v1.inode_v1 != self.inner_v1.identity_v1.inode_v1
            || observed_v1.mode_v1 != self.inner_v1.identity_v1.mode_v1
            || !observed_v1.directory_v1
        {
            return Err(ExplicitFdProcessErrorV1::WorkingDirectoryIdentityDrift);
        }
        Ok(())
    }
}

pub(crate) fn validate_cloexec_read_only_source_v1(
    source_v1: BorrowedFd<'_>,
) -> Result<(), ExplicitFdProcessErrorV1> {
    if !platform::descriptor_is_cloexec_v1(source_v1)? {
        return Err(ExplicitFdProcessErrorV1::SourceDescriptorNotCloseOnExec);
    }
    if !platform::descriptor_is_read_only_v1(source_v1)? {
        return Err(ExplicitFdProcessErrorV1::SourceDescriptorNotReadOnly);
    }
    Ok(())
}

/// Compare the canonical lowercase digest without constructing a hex String.
pub(crate) fn sha256_digest_matches_hex_v1(digest_v1: &[u8; 32], expected_v1: &str) -> bool {
    const HEX_V1: &[u8; 16] = b"0123456789abcdef";
    let expected_v1 = expected_v1.as_bytes();
    if expected_v1.len() != 64 {
        return false;
    }
    digest_v1.iter().enumerate().all(|(index_v1, byte_v1)| {
        expected_v1[index_v1 * 2] == HEX_V1[usize::from(byte_v1 >> 4)]
            && expected_v1[index_v1 * 2 + 1] == HEX_V1[usize::from(byte_v1 & 0x0f)]
    })
}

fn validate_expected_sha256_v1(value_v1: &str) -> Result<(), ExplicitFdProcessErrorV1> {
    if value_v1.len() != 64
        || value_v1.bytes().all(|byte_v1| byte_v1 == b'0')
        || !value_v1
            .bytes()
            .all(|byte_v1| byte_v1.is_ascii_digit() || (b'a'..=b'f').contains(&byte_v1))
    {
        return Err(ExplicitFdProcessErrorV1::InvalidExpectedSha256);
    }
    Ok(())
}
