use std::fmt;
use std::os::fd::RawFd;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExplicitFdProcessErrorV1 {
    SourceDescriptorNotCloseOnExec,
    SourceDescriptorNotReadOnly,
    InvalidExecutableDescriptor,
    InvalidStaticElfExecutable,
    InvalidReadOnlyFileDescriptor,
    InvalidWorkingDirectoryDescriptor,
    InvalidChildPipeDescriptor,
    InvalidChildUnixDatagramDescriptor,
    InvalidRetainedRegularFileDescriptor,
    InvalidInheritedDescriptor,
    InvalidInheritedUnixDatagramDescriptor,
    InvalidInheritedRegularFileDescriptor,
    InvalidInheritedReadOnlyRegularFileDescriptor,
    ExecutableIdentityDrift,
    WorkingDirectoryIdentityDrift,
    InvalidExpectedSha256,
    CustodyDigestMismatch,
    EmptyArgumentVector,
    TooManyBorrowedArguments,
    InvalidBorrowedEnvironment,
    InvalidNativeSourceDescriptors,
    InvalidCString {
        field_v1: &'static str,
    },
    InvalidEnvironmentKey,
    DuplicateEnvironmentKey,
    InvalidChildMemoryLimit,
    ChildMemoryLimitUnavailable,
    Z3SeccompUnavailable,
    InvalidChildDescriptor {
        child_fd_v1: RawFd,
    },
    DuplicateChildDescriptor {
        child_fd_v1: RawFd,
    },
    DescriptorOperation {
        operation_v1: &'static str,
        errno_v1: i32,
    },
    DarwinCustody {
        operation_v1: &'static str,
        errno_v1: i32,
    },
    Spawn {
        operation_v1: &'static str,
        errno_v1: i32,
    },
    ChildSetup {
        stage_v1: u32,
        errno_v1: i32,
    },
    ChildAlreadyWaited,
}

impl fmt::Display for ExplicitFdProcessErrorV1 {
    fn fmt(&self, formatter_v1: &mut fmt::Formatter<'_>) -> fmt::Result {
        if let Some(message_v1) = self.static_message_v1() {
            return formatter_v1.write_str(message_v1);
        }
        match self {
            Self::InvalidCString { field_v1 } => {
                write!(formatter_v1, "{field_v1} contains an interior NUL")
            }
            Self::InvalidChildDescriptor { child_fd_v1 } => {
                write!(formatter_v1, "invalid child descriptor {child_fd_v1}")
            }
            Self::DuplicateChildDescriptor { child_fd_v1 } => {
                write!(formatter_v1, "duplicate child descriptor {child_fd_v1}")
            }
            Self::DescriptorOperation {
                operation_v1,
                errno_v1,
            }
            | Self::DarwinCustody {
                operation_v1,
                errno_v1,
            }
            | Self::Spawn {
                operation_v1,
                errno_v1,
            } => write!(formatter_v1, "{operation_v1} failed with errno {errno_v1}"),
            Self::ChildSetup { stage_v1, errno_v1 } => {
                write!(
                    formatter_v1,
                    "child setup stage {stage_v1} failed with errno {errno_v1}"
                )
            }
            _ => unreachable!("static error variants were handled before structured formatting"),
        }
    }
}

impl ExplicitFdProcessErrorV1 {
    fn static_message_v1(&self) -> Option<&'static str> {
        self.primary_static_message_v1()
            .or_else(|| self.secondary_static_message_v1())
            .or_else(|| self.tertiary_static_message_v1())
    }

    fn primary_static_message_v1(&self) -> Option<&'static str> {
        match self {
            Self::SourceDescriptorNotCloseOnExec => Some("source descriptor must have FD_CLOEXEC"),
            Self::SourceDescriptorNotReadOnly => Some("source descriptor must be read-only"),
            Self::InvalidExecutableDescriptor => {
                Some("executable descriptor is not a bounded executable regular file")
            }
            Self::InvalidStaticElfExecutable => {
                Some("executable is not a static Linux x86-64 ELF64 ET_EXEC image")
            }
            Self::InvalidReadOnlyFileDescriptor => {
                Some("authority descriptor is not a bounded read-only regular file")
            }
            Self::InvalidWorkingDirectoryDescriptor => {
                Some("working-directory descriptor is not a directory")
            }
            Self::InvalidChildPipeDescriptor => Some("child pipe mapping source is not a pipe"),
            Self::InvalidChildUnixDatagramDescriptor => Some(
                "child Unix datagram mapping source must be a connected AF_UNIX SOCK_DGRAM socket",
            ),
            Self::InvalidRetainedRegularFileDescriptor => {
                Some("retained child mapping source must be a read-write regular file")
            }
            _ => None,
        }
    }

    fn secondary_static_message_v1(&self) -> Option<&'static str> {
        match self {
            Self::WorkingDirectoryIdentityDrift => {
                Some("pinned working-directory identity drifted")
            }
            Self::InvalidExpectedSha256 => Some("expected SHA-256 is not canonical"),
            Self::CustodyDigestMismatch => Some("immutable custody SHA-256 mismatched"),
            Self::EmptyArgumentVector => Some("argument vector is empty"),
            Self::TooManyBorrowedArguments => Some("borrowed launch accepts at most six arguments"),
            Self::InvalidBorrowedEnvironment => {
                Some("borrowed environment pointers do not match live entries and final null")
            }
            Self::InvalidEnvironmentKey => Some("environment key is invalid"),
            Self::DuplicateEnvironmentKey => Some("environment key is duplicated"),
            Self::InvalidChildMemoryLimit => Some("child memory limit must be finite and nonzero"),
            Self::ChildMemoryLimitUnavailable => {
                Some("this host cannot install a hard child memory limit before exec")
            }
            Self::Z3SeccompUnavailable => {
                Some("this host cannot enforce the Linux x86-64 Z3 syscall policy")
            }
            Self::ChildAlreadyWaited => Some("child was already waited"),
            Self::InvalidNativeSourceDescriptors => {
                Some("native source requires a CLOEXEC read-only directory and regular database")
            }
            _ => None,
        }
    }

    fn tertiary_static_message_v1(&self) -> Option<&'static str> {
        match self {
            Self::InvalidInheritedDescriptor => Some(
                "inherited child descriptor must be an open read-only non-CLOEXEC pipe at fd >= 3",
            ),
            Self::InvalidInheritedUnixDatagramDescriptor => Some(
                "inherited child descriptor must be a connected AF_UNIX SOCK_DGRAM non-CLOEXEC socket at fd >= 3",
            ),
            Self::InvalidInheritedRegularFileDescriptor => Some(
                "inherited child descriptor must be a read-write non-CLOEXEC regular file at fd >= 3",
            ),
            Self::InvalidInheritedReadOnlyRegularFileDescriptor => Some(
                "inherited child descriptor must be a read-only non-CLOEXEC regular file at fd >= 3",
            ),
            Self::ExecutableIdentityDrift => Some("pinned executable custody identity drifted"),
            _ => None,
        }
    }
}

impl std::error::Error for ExplicitFdProcessErrorV1 {}
