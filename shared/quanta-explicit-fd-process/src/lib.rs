#![deny(unsafe_op_in_unsafe_fn)]

//! Exact process launch from immutable executable-descriptor custody.
//!
//! Parent source descriptors remain close-on-exec. Linux maps descriptors only
//! in the post-fork child and executes a sealed memfd with
//! `execveat(AT_EMPTY_PATH)`. macOS keeps the pinned bytes in immutable private
//! custody, derives a verified non-immutable launch namespace for each spawn, and
//! uses `posix_spawn` file actions with `POSIX_SPAWN_CLOEXEC_DEFAULT` because
//! Darwin exposes no `fexecve`/`execveat` interface.

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
compile_error!("quanta-explicit-fd-process supports only Linux and macOS");

mod borrowed_environment_v1;
mod bounded_exchange_v1;
mod child;
mod error;
mod git_bootstrap_authority_v1;
mod inherited;
mod mutation;
mod pin;
mod pipe;
mod platform;
mod request;
#[cfg(target_os = "linux")]
mod static_elf_v1;

pub use borrowed_environment_v1::BorrowedEnvironmentV1;
pub use bounded_exchange_v1::{
    poll_bounded_exchange_output_v1, poll_bounded_exchange_v1, BoundedExchangeChildV1,
    BoundedExchangeErrorV1, BoundedExchangeExitPolicyV1, BoundedExchangeOutcomeV1,
    BoundedExchangeOutputV1, BoundedExchangePollErrorV1, BoundedExchangeSessionV1,
};
pub use child::ExplicitFdChildV1;
pub use error::ExplicitFdProcessErrorV1;
pub use git_bootstrap_authority_v1::GitProcessBootstrapAuthorityV1;
pub use inherited::{
    try_duplicate_inherited_read_only_regular_file_v1, try_duplicate_inherited_read_pipe_v1,
    try_duplicate_inherited_regular_file_v1, try_duplicate_inherited_unix_datagram_v1,
    try_duplicate_optional_inherited_regular_file_v1,
};
pub use mutation::PathMutationGuardV1;
#[cfg(target_os = "linux")]
pub use pin::ObservedExecutableImageV1;
pub use pin::{PinnedExecutableV1, PinnedReadOnlyFileV1, PinnedWorkingDirectoryV1};
pub use pipe::try_create_cloexec_pipe_v1;
pub use request::ExplicitFdSpawnRequestV1;
pub use request::{
    spawn_borrowed_argv_linux_hard_limited_v1, spawn_borrowed_source_fds_linux_hard_limited_v1,
    spawn_confined_z3_linux_v1, spawn_fixed_two_arg_linux_hard_limited_v1,
};
#[cfg(target_os = "linux")]
pub(crate) use request::{NATIVE_SOURCE_DATABASE_FD_V1, NATIVE_SOURCE_DIRECTORY_FD_V1};
#[cfg(target_os = "linux")]
pub use static_elf_v1::verify_static_linux_x86_64_elf_v1;

use std::os::fd::OwnedFd;

pub const MAX_PINNED_EXECUTABLE_BYTES_V1: u64 = 8 * 1024 * 1024 * 1024;
pub(crate) const MAX_PINNED_READ_ONLY_FILE_BYTES_V1: u64 = 64 * 1024 * 1024;

pub(crate) struct SpawnedProcessV1 {
    pub(crate) pid_v1: libc::pid_t,
    pub(crate) isolated_process_group_v1: bool,
    pub(crate) stdin_v1: OwnedFd,
    pub(crate) stdout_v1: OwnedFd,
    pub(crate) stderr_v1: OwnedFd,
    pub(crate) process_lifetime_guard_v1: Option<platform::ProcessLifetimeGuardV1>,
}

#[cfg(test)]
mod tests;
