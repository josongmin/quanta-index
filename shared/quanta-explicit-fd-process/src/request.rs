use crate::child::ExplicitFdChildV1;
use crate::inherited::{validate_connected_unix_datagram_v1, validate_retained_regular_file_v1};
use crate::pin::validate_cloexec_read_only_source_v1;
use crate::platform;
use crate::{
    BorrowedEnvironmentV1, ExplicitFdProcessErrorV1, PinnedExecutableV1, PinnedReadOnlyFileV1,
    PinnedWorkingDirectoryV1,
};
use std::ffi::{CStr, CString, OsStr};
use std::os::fd::{BorrowedFd, RawFd};
use std::os::unix::ffi::OsStrExt;

const FIRST_CHILD_EXTRA_FD_V1: RawFd = 3;

/// Fixed-shape installed worker launch. Both arguments are borrowed C strings;
/// no argv, environment, descriptor-plan, or child-error Vec is constructed.
/// The caller retains its request-root child-ceiling permit through reap.
pub fn spawn_fixed_two_arg_linux_hard_limited_v1(
    executable_v1: &PinnedExecutableV1,
    arguments_v1: [&CStr; 2],
    bytes_v1: u64,
) -> Result<ExplicitFdChildV1, ExplicitFdProcessErrorV1> {
    spawn_borrowed_argv_linux_hard_limited_v1(
        executable_v1,
        &arguments_v1,
        BorrowedEnvironmentV1::empty_v1(),
        None,
        bytes_v1,
    )
}

/// Fixed stack descriptor/argv plan over admitted caller-owned storage.
/// The argument bound is the certified stack shape, including argv[0]. The
/// caller retains environment, pinned cwd/image, and child-ceiling custody
/// through exchange and reap. No ambient environment is captured here.
pub fn spawn_borrowed_argv_linux_hard_limited_v1(
    executable_v1: &PinnedExecutableV1,
    arguments_v1: &[&CStr],
    environment_v1: BorrowedEnvironmentV1<'_>,
    working_directory_v1: Option<&PinnedWorkingDirectoryV1>,
    bytes_v1: u64,
) -> Result<ExplicitFdChildV1, ExplicitFdProcessErrorV1> {
    spawn_borrowed_native_source_v1(
        executable_v1,
        arguments_v1,
        environment_v1,
        working_directory_v1,
        None,
        bytes_v1,
    )
}

/// Execute the sealed installed solver with only stdin/stdout/stderr, a fixed
/// empty environment, a retained cwd, hard memory limits, and the Linux
/// x86-64 deny-by-default syscall policy. Unsupported hosts refuse admission.
pub fn spawn_confined_z3_linux_v1(
    executable_v1: &PinnedExecutableV1,
    arguments_v1: &[&CStr],
    working_directory_v1: &PinnedWorkingDirectoryV1,
    bytes_v1: u64,
) -> Result<ExplicitFdChildV1, ExplicitFdProcessErrorV1> {
    #[cfg(not(all(target_os = "linux", target_arch = "x86_64")))]
    {
        let _ = (executable_v1, arguments_v1, working_directory_v1, bytes_v1);
        Err(ExplicitFdProcessErrorV1::Z3SeccompUnavailable)
    }
    #[cfg(all(target_os = "linux", target_arch = "x86_64"))]
    {
        if arguments_v1.is_empty() || arguments_v1.len() > 4 {
            return Err(ExplicitFdProcessErrorV1::TooManyBorrowedArguments);
        }
        let ceiling_v1 = libc::rlim_t::try_from(bytes_v1)
            .map_err(|_| ExplicitFdProcessErrorV1::InvalidChildMemoryLimit)?;
        if ceiling_v1 == 0 || ceiling_v1 == libc::RLIM_INFINITY {
            return Err(ExplicitFdProcessErrorV1::InvalidChildMemoryLimit);
        }
        executable_v1.verify_v1()?;
        working_directory_v1.verify_v1()?;
        ExplicitFdChildV1::from_spawned_v1(platform::spawn_borrowed_argv_v1(
            executable_v1,
            arguments_v1,
            BorrowedEnvironmentV1::empty_v1(),
            Some(working_directory_v1),
            None,
            ceiling_v1,
            true,
        ))
    }
}

/// The certified SQLite reader descriptor roles. Parent descriptors remain
/// CLOEXEC; only child fd3/fd4 survive exec. The caller retains admitted source
/// metadata and the original child permit through session teardown and reap.
#[cfg(target_os = "linux")]
pub(crate) const NATIVE_SOURCE_DIRECTORY_FD_V1: RawFd = 3;
#[cfg(target_os = "linux")]
pub(crate) const NATIVE_SOURCE_DATABASE_FD_V1: RawFd = 4;

pub fn spawn_borrowed_source_fds_linux_hard_limited_v1(
    executable_v1: &PinnedExecutableV1,
    arguments_v1: &[&CStr],
    environment_v1: BorrowedEnvironmentV1<'_>,
    working_directory_v1: Option<&PinnedWorkingDirectoryV1>,
    directory_v1: BorrowedFd<'_>,
    database_v1: BorrowedFd<'_>,
    bytes_v1: u64,
) -> Result<ExplicitFdChildV1, ExplicitFdProcessErrorV1> {
    spawn_borrowed_native_source_v1(
        executable_v1,
        arguments_v1,
        environment_v1,
        working_directory_v1,
        Some([directory_v1, database_v1]),
        bytes_v1,
    )
}

fn spawn_borrowed_native_source_v1(
    executable_v1: &PinnedExecutableV1,
    arguments_v1: &[&CStr],
    environment_v1: BorrowedEnvironmentV1<'_>,
    working_directory_v1: Option<&PinnedWorkingDirectoryV1>,
    source_fds_v1: Option<[BorrowedFd<'_>; 2]>,
    bytes_v1: u64,
) -> Result<ExplicitFdChildV1, ExplicitFdProcessErrorV1> {
    #[cfg(not(target_os = "linux"))]
    {
        let _ = (
            executable_v1,
            arguments_v1,
            environment_v1,
            working_directory_v1,
            source_fds_v1,
            bytes_v1,
        );
        Err(ExplicitFdProcessErrorV1::ChildMemoryLimitUnavailable)
    }
    #[cfg(target_os = "linux")]
    {
        if arguments_v1.is_empty() {
            return Err(ExplicitFdProcessErrorV1::EmptyArgumentVector);
        }
        if arguments_v1.len() > 6 {
            return Err(ExplicitFdProcessErrorV1::TooManyBorrowedArguments);
        }
        let ceiling_v1 = libc::rlim_t::try_from(bytes_v1)
            .map_err(|_| ExplicitFdProcessErrorV1::InvalidChildMemoryLimit)?;
        if ceiling_v1 == 0 || ceiling_v1 == libc::RLIM_INFINITY {
            return Err(ExplicitFdProcessErrorV1::InvalidChildMemoryLimit);
        }
        if let Some([directory_v1, database_v1]) = source_fds_v1 {
            validate_cloexec_read_only_source_v1(directory_v1)?;
            validate_cloexec_read_only_source_v1(database_v1)?;
            if !platform::descriptor_identity_v1(directory_v1)?.directory_v1
                || !platform::descriptor_identity_v1(database_v1)?.regular_file_v1
            {
                return Err(ExplicitFdProcessErrorV1::InvalidNativeSourceDescriptors);
            }
        }
        executable_v1.verify_v1()?;
        if let Some(directory_v1) = working_directory_v1 {
            directory_v1.verify_v1()?;
        }
        ExplicitFdChildV1::from_spawned_v1(platform::spawn_borrowed_argv_v1(
            executable_v1,
            arguments_v1,
            environment_v1,
            working_directory_v1,
            source_fds_v1,
            ceiling_v1,
            false,
        ))
    }
}

#[derive(Debug)]
pub(crate) enum ChildMappingSourceV1<'fd> {
    PinnedFile(&'fd PinnedReadOnlyFileV1),
    Pipe(BorrowedFd<'fd>),
    UnixDatagram(BorrowedFd<'fd>),
    RetainedRegularFile(BorrowedFd<'fd>),
}

#[derive(Debug)]
pub(crate) struct ChildFdMappingV1<'fd> {
    pub(crate) source_v1: ChildMappingSourceV1<'fd>,
    pub(crate) child_fd_v1: RawFd,
}

#[derive(Debug)]
pub struct ExplicitFdSpawnRequestV1<'fd> {
    pub(crate) executable_v1: &'fd PinnedExecutableV1,
    pub(crate) argv_v1: Vec<CString>,
    pub(crate) environment_v1: Vec<CString>,
    pub(crate) working_directory_v1: Option<&'fd PinnedWorkingDirectoryV1>,
    pub(crate) mappings_v1: Vec<ChildFdMappingV1<'fd>>,
    pub(crate) isolated_process_group_v1: bool,
    #[cfg(target_os = "linux")]
    pub(crate) linux_hard_memory_limit_v1: Option<u64>,
}

impl<'fd> ExplicitFdSpawnRequestV1<'fd> {
    pub fn try_new_v1<I, A, E, K, V>(
        executable_v1: &'fd PinnedExecutableV1,
        argv_v1: I,
        environment_v1: E,
    ) -> Result<Self, ExplicitFdProcessErrorV1>
    where
        I: IntoIterator<Item = A>,
        A: AsRef<OsStr>,
        E: IntoIterator<Item = (K, V)>,
        K: AsRef<OsStr>,
        V: AsRef<OsStr>,
    {
        let argv_v1 = argv_v1
            .into_iter()
            .map(|argument_v1| cstring_v1(argument_v1.as_ref(), "argv"))
            .collect::<Result<Vec<_>, _>>()?;
        if argv_v1.is_empty() {
            return Err(ExplicitFdProcessErrorV1::EmptyArgumentVector);
        }
        let mut environment_v1 = environment_v1
            .into_iter()
            .map(|(key_v1, value_v1)| environment_entry_v1(key_v1.as_ref(), value_v1.as_ref()))
            .collect::<Result<Vec<_>, _>>()?;
        environment_v1.sort_by(|left_v1, right_v1| left_v1.as_bytes().cmp(right_v1.as_bytes()));
        if environment_v1
            .windows(2)
            .any(|pair_v1| environment_key_v1(&pair_v1[0]) == environment_key_v1(&pair_v1[1]))
        {
            return Err(ExplicitFdProcessErrorV1::DuplicateEnvironmentKey);
        }
        Ok(Self {
            executable_v1,
            argv_v1,
            environment_v1,
            working_directory_v1: None,
            mappings_v1: Vec::new(),
            isolated_process_group_v1: false,
            #[cfg(target_os = "linux")]
            linux_hard_memory_limit_v1: None,
        })
    }

    /// Places the spawned process in a new process group whose identifier is
    /// the child's PID. This lets the owner terminate descendants that retain
    /// inherited pipes. Descendants that deliberately leave the group are not
    /// covered by this custody boundary.
    #[must_use]
    pub fn with_isolated_process_group_v1(mut self) -> Self {
        self.isolated_process_group_v1 = true;
        self
    }

    pub fn with_working_directory_v1(
        mut self,
        working_directory_v1: &'fd PinnedWorkingDirectoryV1,
    ) -> Self {
        self.working_directory_v1 = Some(working_directory_v1);
        self
    }

    /// Installs both address-space and data-segment hard ceilings in the
    /// post-fork child before executing the pinned image. Unsupported hosts
    /// refuse this request instead of silently dropping the ceiling.
    pub fn with_linux_hard_memory_limit_v1(
        self,
        bytes_v1: u64,
    ) -> Result<Self, ExplicitFdProcessErrorV1> {
        #[cfg(not(target_os = "linux"))]
        {
            let _ = (self, bytes_v1);
            Err(ExplicitFdProcessErrorV1::ChildMemoryLimitUnavailable)
        }
        #[cfg(target_os = "linux")]
        {
            let mut self_v1 = self;
            let ceiling_v1 = libc::rlim_t::try_from(bytes_v1)
                .map_err(|_| ExplicitFdProcessErrorV1::InvalidChildMemoryLimit)?;
            if ceiling_v1 == 0 || ceiling_v1 == libc::RLIM_INFINITY {
                return Err(ExplicitFdProcessErrorV1::InvalidChildMemoryLimit);
            }
            self_v1.linux_hard_memory_limit_v1 = Some(bytes_v1);
            Ok(self_v1)
        }
    }

    pub fn with_pinned_read_only_file_v1(
        mut self,
        file_v1: &'fd PinnedReadOnlyFileV1,
        child_fd_v1: RawFd,
    ) -> Result<Self, ExplicitFdProcessErrorV1> {
        validate_new_child_fd_v1(&self.mappings_v1, child_fd_v1)?;
        self.mappings_v1.push(ChildFdMappingV1 {
            source_v1: ChildMappingSourceV1::PinnedFile(file_v1),
            child_fd_v1,
        });
        self.mappings_v1
            .sort_by_key(|mapping_v1| mapping_v1.child_fd_v1);
        Ok(self)
    }

    pub fn with_child_pipe_read_v1(
        mut self,
        pipe_read_v1: BorrowedFd<'fd>,
        child_fd_v1: RawFd,
    ) -> Result<Self, ExplicitFdProcessErrorV1> {
        validate_new_child_fd_v1(&self.mappings_v1, child_fd_v1)?;
        validate_cloexec_read_only_source_v1(pipe_read_v1)?;
        if !platform::descriptor_is_pipe_v1(pipe_read_v1)? {
            return Err(ExplicitFdProcessErrorV1::InvalidChildPipeDescriptor);
        }
        self.mappings_v1.push(ChildFdMappingV1 {
            source_v1: ChildMappingSourceV1::Pipe(pipe_read_v1),
            child_fd_v1,
        });
        self.mappings_v1
            .sort_by_key(|mapping_v1| mapping_v1.child_fd_v1);
        Ok(self)
    }

    /// Maps one connected Unix datagram into the child without making the
    /// parent descriptor ambiently inheritable. The source must remain
    /// CLOEXEC; only the exact child target is opened across exec.
    pub fn with_child_unix_datagram_v1(
        mut self,
        socket_v1: BorrowedFd<'fd>,
        child_fd_v1: RawFd,
    ) -> Result<Self, ExplicitFdProcessErrorV1> {
        validate_new_child_fd_v1(&self.mappings_v1, child_fd_v1)?;
        if !platform::descriptor_is_cloexec_v1(socket_v1)? {
            return Err(ExplicitFdProcessErrorV1::SourceDescriptorNotCloseOnExec);
        }
        validate_connected_unix_datagram_v1(socket_v1)
            .map_err(|_| ExplicitFdProcessErrorV1::InvalidChildUnixDatagramDescriptor)?;
        self.mappings_v1.push(ChildFdMappingV1 {
            source_v1: ChildMappingSourceV1::UnixDatagram(socket_v1),
            child_fd_v1,
        });
        self.mappings_v1
            .sort_by_key(|mapping_v1| mapping_v1.child_fd_v1);
        Ok(self)
    }

    /// Maps a caller-retained read-write regular file into one exact child fd.
    /// The parent source remains CLOEXEC and the caller owns its lifetime.
    /// This mapping conveys a descriptor, not any semantic lock authority.
    pub fn with_child_retained_regular_file_v1(
        mut self,
        file_v1: BorrowedFd<'fd>,
        child_fd_v1: RawFd,
    ) -> Result<Self, ExplicitFdProcessErrorV1> {
        validate_new_child_fd_v1(&self.mappings_v1, child_fd_v1)?;
        if !platform::descriptor_is_cloexec_v1(file_v1)? {
            return Err(ExplicitFdProcessErrorV1::SourceDescriptorNotCloseOnExec);
        }
        validate_retained_regular_file_v1(file_v1)
            .map_err(|_| ExplicitFdProcessErrorV1::InvalidRetainedRegularFileDescriptor)?;
        self.mappings_v1.push(ChildFdMappingV1 {
            source_v1: ChildMappingSourceV1::RetainedRegularFile(file_v1),
            child_fd_v1,
        });
        self.mappings_v1
            .sort_by_key(|mapping_v1| mapping_v1.child_fd_v1);
        Ok(self)
    }

    pub fn spawn_v1(self) -> Result<ExplicitFdChildV1, ExplicitFdProcessErrorV1> {
        if let Some(working_directory_v1) = self.working_directory_v1 {
            working_directory_v1.verify_v1()?;
        }
        self.executable_v1.verify_v1()?;
        for mapping_v1 in &self.mappings_v1 {
            match &mapping_v1.source_v1 {
                ChildMappingSourceV1::PinnedFile(file_v1) => {
                    file_v1.inner_v1.verify_before_spawn_v1()?;
                }
                ChildMappingSourceV1::RetainedRegularFile(file_v1) => {
                    if !platform::descriptor_is_cloexec_v1(*file_v1)? {
                        return Err(ExplicitFdProcessErrorV1::SourceDescriptorNotCloseOnExec);
                    }
                    validate_retained_regular_file_v1(*file_v1).map_err(|_| {
                        ExplicitFdProcessErrorV1::InvalidRetainedRegularFileDescriptor
                    })?;
                }
                _ => {}
            }
        }
        ExplicitFdChildV1::from_spawned_v1(platform::spawn_v1(&self))
    }

    pub(crate) fn mapping_source_descriptor_v1<'mapping>(
        mapping_v1: &'mapping ChildFdMappingV1<'_>,
    ) -> BorrowedFd<'mapping> {
        match &mapping_v1.source_v1 {
            ChildMappingSourceV1::PinnedFile(file_v1) => file_v1.inner_v1.descriptor_v1(),
            ChildMappingSourceV1::Pipe(pipe_v1) => *pipe_v1,
            ChildMappingSourceV1::UnixDatagram(socket_v1) => *socket_v1,
            ChildMappingSourceV1::RetainedRegularFile(file_v1) => *file_v1,
        }
    }
}

fn validate_new_child_fd_v1(
    mappings_v1: &[ChildFdMappingV1<'_>],
    child_fd_v1: RawFd,
) -> Result<(), ExplicitFdProcessErrorV1> {
    if child_fd_v1 < FIRST_CHILD_EXTRA_FD_V1 {
        return Err(ExplicitFdProcessErrorV1::InvalidChildDescriptor { child_fd_v1 });
    }
    if mappings_v1
        .iter()
        .any(|existing_v1| existing_v1.child_fd_v1 == child_fd_v1)
    {
        return Err(ExplicitFdProcessErrorV1::DuplicateChildDescriptor { child_fd_v1 });
    }
    Ok(())
}

fn cstring_v1(
    value_v1: &OsStr,
    field_v1: &'static str,
) -> Result<CString, ExplicitFdProcessErrorV1> {
    CString::new(value_v1.as_bytes())
        .map_err(|_| ExplicitFdProcessErrorV1::InvalidCString { field_v1 })
}

fn environment_entry_v1(
    key_v1: &OsStr,
    value_v1: &OsStr,
) -> Result<CString, ExplicitFdProcessErrorV1> {
    let key_bytes_v1 = key_v1.as_bytes();
    if key_bytes_v1.is_empty() || key_bytes_v1.contains(&b'=') || key_bytes_v1.contains(&0) {
        return Err(ExplicitFdProcessErrorV1::InvalidEnvironmentKey);
    }
    let mut entry_v1 = Vec::with_capacity(key_bytes_v1.len() + value_v1.as_bytes().len() + 1);
    entry_v1.extend_from_slice(key_bytes_v1);
    entry_v1.push(b'=');
    entry_v1.extend_from_slice(value_v1.as_bytes());
    CString::new(entry_v1).map_err(|_| ExplicitFdProcessErrorV1::InvalidCString {
        field_v1: "environment value",
    })
}

fn environment_key_v1(entry_v1: &CString) -> &[u8] {
    match entry_v1.as_bytes().split(|byte_v1| *byte_v1 == b'=').next() {
        Some(key_v1) => key_v1,
        None => entry_v1.as_bytes(),
    }
}
