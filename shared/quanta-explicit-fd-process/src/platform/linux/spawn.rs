//! Audited post-fork transaction. The child path uses only async-signal-safe
//! raw syscalls over parent-prepared storage and never mutates parent FD flags.

use super::{descriptor_error_v1, duplicate_cloexec_at_least_v1, errno_v1, spawn_error_v1};
use crate::{
    BorrowedEnvironmentV1, ExplicitFdProcessErrorV1, ExplicitFdSpawnRequestV1, PinnedExecutableV1,
    PinnedWorkingDirectoryV1, SpawnedProcessV1,
};
use std::ffi::{c_char, CStr, CString};
use std::io::Read;
use std::os::fd::{AsFd, AsRawFd, BorrowedFd, FromRawFd, OwnedFd, RawFd};

const CHILD_STAGE_DUP_V1: u32 = 1;
const CHILD_STAGE_CLOSE_RANGE_V1: u32 = 2;
const CHILD_STAGE_EXEC_V1: u32 = 3;
const CHILD_STAGE_FCHDIR_V1: u32 = 4;
const CHILD_STAGE_PROCESS_GROUP_V1: u32 = 5;
const CHILD_STAGE_MEMORY_LIMIT_V1: u32 = 6;
const CHILD_STAGE_Z3_SECCOMP_V1: u32 = 7;
const CHILD_ERROR_BYTES_V1: usize = 8;
const CHILD_ERROR_READ_LIMIT_V1: u64 = 9;

pub(super) fn spawn_v1(
    request_v1: &ExplicitFdSpawnRequestV1<'_>,
) -> Result<SpawnedProcessV1, ExplicitFdProcessErrorV1> {
    let pipes_v1 = ParentPipesV1::try_new_v1()?;
    let sources_v1 = ChildSourcesV1::try_new_v1(request_v1, &pipes_v1)?;
    let mapping_ops_v1 = mapping_operations_v1(
        &sources_v1.stdin_v1,
        &sources_v1.stdout_v1,
        &sources_v1.stderr_v1,
        &sources_v1.extras_v1,
        request_v1,
    );
    let kept_descriptors_v1 = kept_descriptors_v1(
        sources_v1.executable_v1.as_raw_fd(),
        sources_v1.error_v1.as_raw_fd(),
        sources_v1.cwd_v1.as_ref(),
        request_v1,
    );
    let argv_pointers_v1 = c_pointer_vector_v1(&request_v1.argv_v1);
    let environment_pointers_v1 = c_pointer_vector_v1(&request_v1.environment_v1);
    let plan_v1 = ChildExecPlanV1 {
        executable_v1: sources_v1.executable_v1.as_raw_fd(),
        working_directory_v1: sources_v1.cwd_v1.as_ref().map(|cwd_v1| cwd_v1.as_raw_fd()),
        mappings_v1: &mapping_ops_v1,
        kept_descriptors_v1: &kept_descriptors_v1,
        error_descriptor_v1: sources_v1.error_v1.as_raw_fd(),
        argv_v1: argv_pointers_v1.as_ptr(),
        environment_v1: environment_pointers_v1.as_ptr(),
        isolated_process_group_v1: request_v1.isolated_process_group_v1,
        hard_memory_limit_v1: request_v1.linux_hard_memory_limit_v1.map(|bytes_v1| {
            let ceiling_v1 = bytes_v1 as libc::rlim_t;
            libc::rlimit {
                rlim_cur: ceiling_v1,
                rlim_max: ceiling_v1,
            }
        }),
        z3_seccomp_filter_v1: None,
    };
    // SAFETY: the child branch enters only the audited async-signal-safe path.
    let pid_v1 = unsafe { libc::fork() };
    if pid_v1 < 0 {
        return Err(spawn_error_v1("fork_v1"));
    }
    if pid_v1 == 0 {
        // SAFETY: all referenced parent-prepared buffers remain live until exec/_exit.
        unsafe { child_exec_v1(&plan_v1) }
    }
    drop(sources_v1);
    pipes_v1.finish_after_fork_v1(pid_v1, request_v1.isolated_process_group_v1)
}

/// Heap-free parent plan for certified borrowed caller shapes. Fixed arrays are
/// prepared before fork and inherited immutably by the async-signal-safe child.
pub(super) fn spawn_borrowed_argv_v1(
    executable_v1: &PinnedExecutableV1,
    arguments_v1: &[&CStr],
    environment_v1: BorrowedEnvironmentV1<'_>,
    working_directory_v1: Option<&PinnedWorkingDirectoryV1>,
    source_fds_v1: Option<[BorrowedFd<'_>; 2]>,
    ceiling_v1: libc::rlim_t,
    confined_z3_v1: bool,
) -> Result<SpawnedProcessV1, ExplicitFdProcessErrorV1> {
    let pipes_v1 = ParentPipesV1::try_new_v1()?;
    let fixed_sources_v1 = FixedChildSourcesV1::try_new_v1(
        executable_v1,
        working_directory_v1,
        source_fds_v1,
        &pipes_v1,
    )?;
    let sources_v1 = &fixed_sources_v1.base_v1;
    let z3_seccomp_filter_v1: Option<Vec<libc::sock_filter>> = if confined_z3_v1 {
        #[cfg(target_arch = "x86_64")]
        {
            Some(super::solver_seccomp_v1::filter_v1(sources_v1.executable_v1.as_raw_fd()))
        }
        #[cfg(not(target_arch = "x86_64"))]
        {
            return Err(ExplicitFdProcessErrorV1::Z3SeccompUnavailable);
        }
    } else {
        None
    };
    let mappings_v1 = [
        MappingOpV1 {
            source_v1: sources_v1.stdin_v1.as_raw_fd(),
            destination_v1: libc::STDIN_FILENO,
        },
        MappingOpV1 {
            source_v1: sources_v1.stdout_v1.as_raw_fd(),
            destination_v1: libc::STDOUT_FILENO,
        },
        MappingOpV1 {
            source_v1: sources_v1.stderr_v1.as_raw_fd(),
            destination_v1: libc::STDERR_FILENO,
        },
        MappingOpV1 {
            source_v1: fixed_sources_v1.source_fds_v1[0]
                .as_ref()
                .map_or(-1, AsRawFd::as_raw_fd),
            destination_v1: crate::NATIVE_SOURCE_DIRECTORY_FD_V1,
        },
        MappingOpV1 {
            source_v1: fixed_sources_v1.source_fds_v1[1]
                .as_ref()
                .map_or(-1, AsRawFd::as_raw_fd),
            destination_v1: crate::NATIVE_SOURCE_DATABASE_FD_V1,
        },
    ];
    let mapping_len_v1 = if source_fds_v1.is_some() { 5 } else { 3 };
    let mut kept_v1 = [-1; 8];
    kept_v1[..5].copy_from_slice(&[
        libc::STDIN_FILENO,
        libc::STDOUT_FILENO,
        libc::STDERR_FILENO,
        sources_v1.executable_v1.as_raw_fd(),
        sources_v1.error_v1.as_raw_fd(),
    ]);
    let mut kept_len_v1 = 5;
    if source_fds_v1.is_some() {
        kept_v1[5] = crate::NATIVE_SOURCE_DIRECTORY_FD_V1;
        kept_v1[6] = crate::NATIVE_SOURCE_DATABASE_FD_V1;
        kept_len_v1 = 7;
    }
    if let Some(cwd_v1) = sources_v1.cwd_v1.as_ref() {
        kept_v1[kept_len_v1] = cwd_v1.as_raw_fd();
        kept_len_v1 += 1;
    }
    kept_v1[..kept_len_v1].sort_unstable();
    let mut argv_v1 = [std::ptr::null_mut(); 7];
    for (slot_v1, argument_v1) in argv_v1.iter_mut().zip(arguments_v1) {
        *slot_v1 = argument_v1.as_ptr().cast_mut();
    }
    let limit_v1 = libc::rlimit {
        rlim_cur: ceiling_v1,
        rlim_max: ceiling_v1,
    };
    let plan_v1 = ChildExecPlanV1 {
        executable_v1: sources_v1.executable_v1.as_raw_fd(),
        working_directory_v1: sources_v1.cwd_v1.as_ref().map(|cwd_v1| cwd_v1.as_raw_fd()),
        mappings_v1: &mappings_v1[..mapping_len_v1],
        kept_descriptors_v1: &kept_v1[..kept_len_v1],
        error_descriptor_v1: sources_v1.error_v1.as_raw_fd(),
        argv_v1: argv_v1.as_ptr(),
        environment_v1: environment_v1.pointers_v1().as_ptr(),
        isolated_process_group_v1: true,
        hard_memory_limit_v1: Some(limit_v1),
        z3_seccomp_filter_v1: z3_seccomp_filter_v1.as_deref(),
    };
    // SAFETY: the child enters only child_exec_v1 over stack-prepared storage.
    let pid_v1 = unsafe { libc::fork() };
    if pid_v1 < 0 {
        return Err(spawn_error_v1("fork_fixed_worker_v1"));
    }
    if pid_v1 == 0 {
        // SAFETY: all referenced arrays and descriptors remain live in the
        // forked address space until execveat or _exit.
        unsafe { child_exec_v1(&plan_v1) }
    }
    drop(fixed_sources_v1);
    pipes_v1.finish_after_fork_v1(pid_v1, true)
}

struct ParentPipesV1 {
    stdin_read_v1: OwnedFd,
    stdin_write_v1: OwnedFd,
    stdout_read_v1: OwnedFd,
    stdout_write_v1: OwnedFd,
    stderr_read_v1: OwnedFd,
    stderr_write_v1: OwnedFd,
    error_read_v1: OwnedFd,
    error_write_v1: OwnedFd,
}

impl ParentPipesV1 {
    fn try_new_v1() -> Result<Self, ExplicitFdProcessErrorV1> {
        let (stdin_read_v1, stdin_write_v1) = pipe_cloexec_v1()?;
        let (stdout_read_v1, stdout_write_v1) = pipe_cloexec_v1()?;
        let (stderr_read_v1, stderr_write_v1) = pipe_cloexec_v1()?;
        let (error_read_v1, error_write_v1) = pipe_cloexec_v1()?;
        Ok(Self {
            stdin_read_v1,
            stdin_write_v1,
            stdout_read_v1,
            stdout_write_v1,
            stderr_read_v1,
            stderr_write_v1,
            error_read_v1,
            error_write_v1,
        })
    }

    fn finish_after_fork_v1(
        self,
        pid_v1: libc::pid_t,
        isolated_process_group_v1: bool,
    ) -> Result<SpawnedProcessV1, ExplicitFdProcessErrorV1> {
        let Self {
            stdin_read_v1,
            stdin_write_v1,
            stdout_read_v1,
            stdout_write_v1,
            stderr_read_v1,
            stderr_write_v1,
            error_read_v1,
            error_write_v1,
        } = self;
        drop(stdin_read_v1);
        drop(stdout_write_v1);
        drop(stderr_write_v1);
        // Parent EOF proves exec success only after every parent copy of the
        // CLOEXEC child-error writer is closed here.
        drop(error_write_v1);
        let child_error_v1 = match read_child_error_v1(error_read_v1) {
            Ok(error_v1) => error_v1,
            Err(error_v1) => {
                super::terminate_and_reap_v1(pid_v1, isolated_process_group_v1);
                return Err(error_v1);
            }
        };
        if let Some(error_v1) = child_error_v1 {
            reap_failed_child_v1(pid_v1);
            return Err(error_v1);
        }
        Ok(SpawnedProcessV1 {
            pid_v1,
            isolated_process_group_v1,
            stdin_v1: stdin_write_v1,
            stdout_v1: stdout_read_v1,
            stderr_v1: stderr_read_v1,
            process_lifetime_guard_v1: None,
        })
    }
}

struct ChildSourcesV1 {
    executable_v1: OwnedFd,
    stdin_v1: OwnedFd,
    stdout_v1: OwnedFd,
    stderr_v1: OwnedFd,
    error_v1: OwnedFd,
    cwd_v1: Option<OwnedFd>,
    extras_v1: Vec<OwnedFd>,
}

struct FixedChildSourcesV1 {
    base_v1: ChildSourcesV1,
    source_fds_v1: [Option<OwnedFd>; 2],
}
impl FixedChildSourcesV1 {
    fn try_new_v1(
        executable_v1: &PinnedExecutableV1,
        working_directory_v1: Option<&PinnedWorkingDirectoryV1>,
        source_fds_v1: Option<[BorrowedFd<'_>; 2]>,
        pipes_v1: &ParentPipesV1,
    ) -> Result<Self, ExplicitFdProcessErrorV1> {
        let highest_target_v1 = if source_fds_v1.is_some() {
            crate::NATIVE_SOURCE_DATABASE_FD_V1
        } else {
            2
        };
        let mut cursor_v1 = DuplicateCursorV1::try_new_v1(highest_target_v1)?;
        let base_v1 = ChildSourcesV1 {
            executable_v1: cursor_v1.duplicate_v1(
                executable_v1
                    .inner_v1
                    .platform_v1
                    .custody_v1
                    .descriptor_v1
                    .as_fd(),
            )?,
            stdin_v1: cursor_v1.duplicate_v1(pipes_v1.stdin_read_v1.as_fd())?,
            stdout_v1: cursor_v1.duplicate_v1(pipes_v1.stdout_write_v1.as_fd())?,
            stderr_v1: cursor_v1.duplicate_v1(pipes_v1.stderr_write_v1.as_fd())?,
            error_v1: cursor_v1.duplicate_v1(pipes_v1.error_write_v1.as_fd())?,
            cwd_v1: working_directory_v1
                .map(|directory_v1| {
                    cursor_v1.duplicate_v1(directory_v1.inner_v1.descriptor_v1.as_fd())
                })
                .transpose()?,
            extras_v1: Vec::new(),
        };
        let source_fds_v1 = if let Some([directory_v1, database_v1]) = source_fds_v1 {
            [
                Some(cursor_v1.duplicate_v1(directory_v1)?),
                Some(cursor_v1.duplicate_v1(database_v1)?),
            ]
        } else {
            [None, None]
        };
        Ok(Self {
            base_v1,
            source_fds_v1,
        })
    }
}

impl ChildSourcesV1 {
    fn try_new_v1(
        request_v1: &ExplicitFdSpawnRequestV1<'_>,
        pipes_v1: &ParentPipesV1,
    ) -> Result<Self, ExplicitFdProcessErrorV1> {
        let highest_target_v1 = request_v1
            .mappings_v1
            .iter()
            .map(|mapping_v1| mapping_v1.child_fd_v1)
            .max()
            .unwrap_or(libc::STDERR_FILENO);
        let mut cursor_v1 = DuplicateCursorV1::try_new_v1(highest_target_v1)?;
        let executable_v1 = cursor_v1.duplicate_v1(
            request_v1
                .executable_v1
                .inner_v1
                .platform_v1
                .custody_v1
                .descriptor_v1
                .as_fd(),
        )?;
        let stdin_v1 = cursor_v1.duplicate_v1(pipes_v1.stdin_read_v1.as_fd())?;
        let stdout_v1 = cursor_v1.duplicate_v1(pipes_v1.stdout_write_v1.as_fd())?;
        let stderr_v1 = cursor_v1.duplicate_v1(pipes_v1.stderr_write_v1.as_fd())?;
        let error_v1 = cursor_v1.duplicate_v1(pipes_v1.error_write_v1.as_fd())?;
        let cwd_v1 = match request_v1.working_directory_v1 {
            Some(directory_v1) => {
                Some(cursor_v1.duplicate_v1(directory_v1.inner_v1.descriptor_v1.as_fd())?)
            }
            None => None,
        };
        let extras_v1 = duplicate_extra_sources_v1(request_v1, &mut cursor_v1)?;
        Ok(Self {
            executable_v1,
            stdin_v1,
            stdout_v1,
            stderr_v1,
            error_v1,
            cwd_v1,
            extras_v1,
        })
    }
}

struct DuplicateCursorV1 {
    next_v1: RawFd,
}

impl DuplicateCursorV1 {
    fn try_new_v1(highest_target_v1: RawFd) -> Result<Self, ExplicitFdProcessErrorV1> {
        Ok(Self {
            next_v1: highest_target_v1.checked_add(1).ok_or(
                ExplicitFdProcessErrorV1::InvalidChildDescriptor {
                    child_fd_v1: highest_target_v1,
                },
            )?,
        })
    }

    fn duplicate_v1(
        &mut self,
        source_v1: BorrowedFd<'_>,
    ) -> Result<OwnedFd, ExplicitFdProcessErrorV1> {
        let duplicate_v1 = duplicate_cloexec_at_least_v1(source_v1, self.next_v1)?;
        self.next_v1 = next_descriptor_v1(duplicate_v1.as_raw_fd())?;
        Ok(duplicate_v1)
    }
}

fn duplicate_extra_sources_v1(
    request_v1: &ExplicitFdSpawnRequestV1<'_>,
    cursor_v1: &mut DuplicateCursorV1,
) -> Result<Vec<OwnedFd>, ExplicitFdProcessErrorV1> {
    request_v1
        .mappings_v1
        .iter()
        .map(|mapping_v1| match &mapping_v1.source_v1 {
            crate::request::ChildMappingSourceV1::PinnedFile(file_v1) => {
                let opened_v1 = file_v1.inner_v1.try_open_read_only_for_spawn_v1()?;
                cursor_v1.duplicate_v1(opened_v1.as_fd())
            }
            _ => cursor_v1
                .duplicate_v1(ExplicitFdSpawnRequestV1::mapping_source_descriptor_v1(mapping_v1)),
        })
        .collect()
}

fn c_pointer_vector_v1(values_v1: &[CString]) -> Vec<*mut c_char> {
    let mut pointers_v1 = values_v1
        .iter()
        .map(|value_v1| value_v1.as_ptr().cast_mut())
        .collect::<Vec<_>>();
    pointers_v1.push(std::ptr::null_mut());
    pointers_v1
}

fn mapping_operations_v1(
    stdin_v1: &OwnedFd,
    stdout_v1: &OwnedFd,
    stderr_v1: &OwnedFd,
    extra_sources_v1: &[OwnedFd],
    request_v1: &ExplicitFdSpawnRequestV1<'_>,
) -> Vec<MappingOpV1> {
    let mut operations_v1 = vec![
        MappingOpV1 {
            source_v1: stdin_v1.as_raw_fd(),
            destination_v1: libc::STDIN_FILENO,
        },
        MappingOpV1 {
            source_v1: stdout_v1.as_raw_fd(),
            destination_v1: libc::STDOUT_FILENO,
        },
        MappingOpV1 {
            source_v1: stderr_v1.as_raw_fd(),
            destination_v1: libc::STDERR_FILENO,
        },
    ];
    operations_v1.extend(extra_sources_v1.iter().zip(&request_v1.mappings_v1).map(
        |(source_v1, mapping_v1)| MappingOpV1 {
            source_v1: source_v1.as_raw_fd(),
            destination_v1: mapping_v1.child_fd_v1,
        },
    ));
    operations_v1
}

fn kept_descriptors_v1(
    executable_v1: RawFd,
    error_v1: RawFd,
    cwd_v1: Option<&OwnedFd>,
    request_v1: &ExplicitFdSpawnRequestV1<'_>,
) -> Vec<RawFd> {
    let mut kept_v1 = request_v1
        .mappings_v1
        .iter()
        .map(|mapping_v1| mapping_v1.child_fd_v1)
        .chain([
            libc::STDIN_FILENO,
            libc::STDOUT_FILENO,
            libc::STDERR_FILENO,
            executable_v1,
            error_v1,
        ])
        .chain(cwd_v1.iter().map(|cwd_v1| cwd_v1.as_raw_fd()))
        .collect::<Vec<_>>();
    kept_v1.sort_unstable();
    kept_v1.dedup();
    kept_v1
}

fn next_descriptor_v1(current_v1: RawFd) -> Result<RawFd, ExplicitFdProcessErrorV1> {
    current_v1
        .checked_add(1)
        .ok_or(ExplicitFdProcessErrorV1::DescriptorOperation {
            operation_v1: "descriptor_range_v1",
            errno_v1: libc::EMFILE,
        })
}

#[derive(Clone, Copy)]
struct MappingOpV1 {
    source_v1: RawFd,
    destination_v1: RawFd,
}

struct ChildExecPlanV1<'plan> {
    executable_v1: RawFd,
    working_directory_v1: Option<RawFd>,
    mappings_v1: &'plan [MappingOpV1],
    kept_descriptors_v1: &'plan [RawFd],
    error_descriptor_v1: RawFd,
    argv_v1: *const *mut c_char,
    environment_v1: *const *mut c_char,
    isolated_process_group_v1: bool,
    hard_memory_limit_v1: Option<libc::rlimit>,
    z3_seccomp_filter_v1: Option<&'plan [libc::sock_filter]>,
}

/// # Safety
/// Called only after fork with immutable parent-prepared storage. It invokes
/// only async-signal-safe raw syscalls and terminates through exec or `_exit`.
// SAFETY: caller enters only from the post-fork child with prepared storage.
unsafe fn child_exec_v1(plan_v1: &ChildExecPlanV1<'_>) -> ! {
    if let Some(limit_v1) = plan_v1.hard_memory_limit_v1.as_ref() {
        // SAFETY: setrlimit is async-signal-safe and uses parent-prepared
        // values. Failure is reported before execveat can start native work.
        if unsafe {
            libc::setrlimit(libc::RLIMIT_AS, limit_v1) != 0
                || libc::setrlimit(libc::RLIMIT_DATA, limit_v1) != 0
        } {
            // SAFETY: this branch reports a fixed parent-prepared error and
            // exits the child without entering the executable image.
            unsafe {
                child_fail_v1(
                    plan_v1.error_descriptor_v1,
                    CHILD_STAGE_MEMORY_LIMIT_V1,
                    child_errno_v1(),
                )
            }
        }
    }
    if plan_v1.isolated_process_group_v1 {
        // SAFETY: setpgid is async-signal-safe and 0/0 makes this child the
        // leader of a fresh process group before any descendant can exist.
        if unsafe { libc::setpgid(0, 0) } != 0 {
            // SAFETY: fixed error payload over the parent-prepared pipe.
            unsafe {
                child_fail_v1(
                    plan_v1.error_descriptor_v1,
                    CHILD_STAGE_PROCESS_GROUP_V1,
                    child_errno_v1(),
                )
            }
        }
    }
    // SAFETY: both helpers use only async-signal-safe syscalls and prepared data.
    unsafe { child_apply_working_directory_v1(plan_v1) };
    // SAFETY: mapping sources and destinations are parent-prepared and disjoint.
    unsafe { child_apply_mappings_v1(plan_v1) };
    // SAFETY: closes only ranges excluded from the sorted keep set.
    if let Err(errno_v1) = unsafe { close_unintended_descriptors_v1(plan_v1.kept_descriptors_v1) } {
        // SAFETY: fixed error payload over parent-prepared pipe.
        unsafe { child_fail_v1(plan_v1.error_descriptor_v1, CHILD_STAGE_CLOSE_RANGE_V1, errno_v1) }
    }
    if let Some(filter_v1) = plan_v1.z3_seccomp_filter_v1 {
        let program_v1 = libc::sock_fprog {
            len: u16::try_from(filter_v1.len()).unwrap_or(0),
            filter: filter_v1.as_ptr().cast_mut(),
        };
        // SAFETY: the child has only parent-prepared descriptors and BPF bytes.
        // A missing seccomp facility is a child-setup error, never a fallback.
        if unsafe {
            libc::prctl(libc::PR_SET_NO_NEW_PRIVS, 1, 0, 0, 0) != 0
                || libc::prctl(
                    libc::PR_SET_SECCOMP,
                    libc::SECCOMP_MODE_FILTER,
                    &program_v1 as *const libc::sock_fprog,
                ) != 0
        } {
            // SAFETY: fixed error payload over the parent-prepared pipe.
            unsafe {
                child_fail_v1(
                    plan_v1.error_descriptor_v1,
                    CHILD_STAGE_Z3_SECCOMP_V1,
                    child_errno_v1(),
                )
            }
        }
    }
    static EMPTY_PATH_V1: [u8; 1] = [0];
    // SAFETY: arrays are NUL-terminated and sealed executable custody is live.
    unsafe {
        libc::syscall(
            libc::SYS_execveat,
            plan_v1.executable_v1,
            EMPTY_PATH_V1.as_ptr().cast::<c_char>(),
            plan_v1.argv_v1,
            plan_v1.environment_v1,
            libc::AT_EMPTY_PATH,
        )
    };
    // SAFETY: execveat returned only on failure.
    unsafe { child_fail_v1(plan_v1.error_descriptor_v1, CHILD_STAGE_EXEC_V1, child_errno_v1()) }
}

// SAFETY: called only from the audited post-fork child transaction.
unsafe fn child_apply_working_directory_v1(plan_v1: &ChildExecPlanV1<'_>) {
    let Some(working_directory_v1) = plan_v1.working_directory_v1 else {
        return;
    };
    // SAFETY: fchdir is async-signal-safe and the descriptor is pinned.
    if unsafe { libc::fchdir(working_directory_v1) } != 0 {
        // SAFETY: fixed error payload over parent-prepared pipe.
        unsafe {
            child_fail_v1(plan_v1.error_descriptor_v1, CHILD_STAGE_FCHDIR_V1, child_errno_v1())
        }
    }
}

// SAFETY: called only from the audited post-fork child transaction.
unsafe fn child_apply_mappings_v1(plan_v1: &ChildExecPlanV1<'_>) {
    for mapping_v1 in plan_v1.mappings_v1 {
        // SAFETY: dup3 is async-signal-safe and source/targets are disjoint.
        if unsafe { libc::dup3(mapping_v1.source_v1, mapping_v1.destination_v1, 0) } < 0 {
            // SAFETY: fixed error payload over parent-prepared pipe.
            unsafe {
                child_fail_v1(plan_v1.error_descriptor_v1, CHILD_STAGE_DUP_V1, child_errno_v1())
            }
        }
    }
}

// SAFETY: called only from the audited post-fork child transaction.
unsafe fn close_unintended_descriptors_v1(kept_v1: &[RawFd]) -> Result<(), i32> {
    let mut cursor_v1 = 3_u32;
    for descriptor_v1 in kept_v1 {
        if *descriptor_v1 < 3 {
            continue;
        }
        let keep_v1 = u32::try_from(*descriptor_v1).map_err(|_| libc::EINVAL)?;
        if cursor_v1 < keep_v1 {
            // SAFETY: close_range is a raw syscall over the inclusive range.
            if unsafe { libc::syscall(libc::SYS_close_range, cursor_v1, keep_v1 - 1, 0_u32) } < 0 {
                return Err(child_errno_v1());
            }
        }
        cursor_v1 = keep_v1.saturating_add(1);
    }
    // SAFETY: final range closes every higher unintended descriptor.
    if unsafe { libc::syscall(libc::SYS_close_range, cursor_v1, u32::MAX, 0_u32) } < 0 {
        return Err(child_errno_v1());
    }
    Ok(())
}

// SAFETY: called only from the audited post-fork child transaction.
unsafe fn child_fail_v1(error_descriptor_v1: RawFd, stage_v1: u32, errno_value_v1: i32) -> ! {
    let stage_bytes_v1 = stage_v1.to_ne_bytes();
    let errno_bytes_v1 = errno_value_v1.to_ne_bytes();
    let bytes_v1 = [
        stage_bytes_v1[0],
        stage_bytes_v1[1],
        stage_bytes_v1[2],
        stage_bytes_v1[3],
        errno_bytes_v1[0],
        errno_bytes_v1[1],
        errno_bytes_v1[2],
        errno_bytes_v1[3],
    ];
    let mut written_v1 = 0_usize;
    while written_v1 < bytes_v1.len() {
        // SAFETY: written_v1 is bounded by bytes_v1.len(); add therefore points
        // into the fixed stack buffer and write never receives a larger length.
        let result_v1 = unsafe {
            libc::write(
                error_descriptor_v1,
                bytes_v1.as_ptr().add(written_v1).cast(),
                bytes_v1.len() - written_v1,
            )
        };
        if result_v1 > 0 {
            let count_v1 = result_v1 as usize;
            if count_v1 > bytes_v1.len() - written_v1 {
                break;
            }
            written_v1 += count_v1;
        } else if result_v1 < 0 && child_errno_v1() == libc::EINTR {
            continue;
        } else {
            break;
        }
    }
    // SAFETY: only valid termination after failed child setup.
    unsafe { libc::_exit(127) }
}

fn pipe_cloexec_v1() -> Result<(OwnedFd, OwnedFd), ExplicitFdProcessErrorV1> {
    let mut descriptors_v1 = [-1, -1];
    // SAFETY: pipe2 initializes both descriptors atomically with CLOEXEC.
    if unsafe { libc::pipe2(descriptors_v1.as_mut_ptr(), libc::O_CLOEXEC) } != 0 {
        return Err(descriptor_error_v1("pipe2_cloexec_v1"));
    }
    // SAFETY: pipe2 returned two fresh owned descriptors.
    let read_v1 = unsafe { OwnedFd::from_raw_fd(descriptors_v1[0]) };
    // SAFETY: pipe2 returned two fresh owned descriptors.
    let write_v1 = unsafe { OwnedFd::from_raw_fd(descriptors_v1[1]) };
    Ok((read_v1, write_v1))
}

fn read_child_error_v1(
    descriptor_v1: OwnedFd,
) -> Result<Option<ExplicitFdProcessErrorV1>, ExplicitFdProcessErrorV1> {
    let mut file_v1 = std::fs::File::from(descriptor_v1);
    let mut bytes_v1 = [0_u8; CHILD_ERROR_READ_LIMIT_V1 as usize];
    let mut received_v1 = 0_usize;
    while received_v1 < bytes_v1.len() {
        match file_v1.read(&mut bytes_v1[received_v1..]) {
            Ok(0) => break,
            Ok(read_v1) => received_v1 += read_v1,
            Err(error_v1) if error_v1.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(_) => return Err(spawn_error_v1("read_child_error_v1")),
        }
    }
    if received_v1 == 0 {
        return Ok(None);
    }
    if received_v1 != CHILD_ERROR_BYTES_V1 {
        return Err(ExplicitFdProcessErrorV1::Spawn {
            operation_v1: "malformed_child_error_v1",
            errno_v1: libc::EPROTO,
        });
    }
    let stage_v1 = u32::from_ne_bytes(
        bytes_v1[..4]
            .try_into()
            .map_err(|_| spawn_error_v1("decode_child_error_stage_v1"))?,
    );
    let errno_v1 = i32::from_ne_bytes(
        bytes_v1[4..CHILD_ERROR_BYTES_V1]
            .try_into()
            .map_err(|_| spawn_error_v1("decode_child_error_errno_v1"))?,
    );
    Ok(Some(ExplicitFdProcessErrorV1::ChildSetup { stage_v1, errno_v1 }))
}

pub(super) fn reap_failed_child_v1(pid_v1: libc::pid_t) {
    let mut status_v1 = 0;
    loop {
        // SAFETY: best-effort reap of known child.
        let result_v1 = unsafe { libc::waitpid(pid_v1, &mut status_v1, 0) };
        if result_v1 == pid_v1 || (result_v1 < 0 && errno_v1() != libc::EINTR) {
            break;
        }
    }
}

fn child_errno_v1() -> i32 {
    // SAFETY: read thread-local errno immediately after raw syscall failure.
    unsafe { *libc::__errno_location() }
}
