use super::fifo::DarwinStdioFifosV1;
use super::{duplicate_cloexec_at_least_v1, DarwinEphemeralLaunchImageV1};
use crate::{ExplicitFdProcessErrorV1, ExplicitFdSpawnRequestV1, SpawnedProcessV1};
use std::ffi::{c_char, c_void, CString};
use std::os::fd::{AsFd, AsRawFd, OwnedFd};

pub(super) fn spawn_v1(
    request_v1: &ExplicitFdSpawnRequestV1<'_>,
) -> Result<SpawnedProcessV1, ExplicitFdProcessErrorV1> {
    let launch_image_v1 = request_v1
        .executable_v1
        .inner_v1
        .platform_v1
        .try_prepare_launch_image_v1()?;
    let stdio_v1 = DarwinStdioFifosV1::try_new_v1()?;
    let sources_v1 = prepare_sources_v1(request_v1)?;
    let actions_v1 = prepare_actions_v1(request_v1, &stdio_v1, &sources_v1)?;
    let attributes_v1 = SpawnAttributesV1::try_new_v1(request_v1.isolated_process_group_v1)?;
    let argv_pointers_v1 = c_pointer_vector_v1(&request_v1.argv_v1);
    let environment_pointers_v1 = c_pointer_vector_v1(&request_v1.environment_v1);
    stdio_v1.verify_before_spawn_v1()?;
    request_v1.executable_v1.verify_v1()?;
    launch_image_v1.verify_before_spawn_v1()?;
    let pid_v1 = call_posix_spawn_v1(
        &launch_image_v1,
        &actions_v1,
        &attributes_v1,
        &argv_pointers_v1,
        &environment_pointers_v1,
    )?;
    drop(actions_v1);
    drop(attributes_v1);
    drop(sources_v1);
    match stdio_v1.finish_after_spawn_v1(pid_v1) {
        Ok(mut spawned_v1) => {
            // Darwin dyld can continue loading through the launch pathname
            // after posix_spawn returns. Transfer namespace ownership to the
            // child so cleanup occurs only after wait/reap or kill/reap.
            spawned_v1.process_lifetime_guard_v1 = Some(launch_image_v1);
            spawned_v1.isolated_process_group_v1 = request_v1.isolated_process_group_v1;
            Ok(spawned_v1)
        }
        Err(error_v1) => {
            super::terminate_and_reap_v1(pid_v1, request_v1.isolated_process_group_v1);
            drop(launch_image_v1);
            Err(error_v1)
        }
    }
}

struct SpawnSourcesV1 {
    cwd_v1: Option<OwnedFd>,
    extras_v1: Vec<OwnedFd>,
}

fn prepare_sources_v1(
    request_v1: &ExplicitFdSpawnRequestV1<'_>,
) -> Result<SpawnSourcesV1, ExplicitFdProcessErrorV1> {
    let highest_target_v1 = request_v1
        .mappings_v1
        .iter()
        .map(|mapping_v1| mapping_v1.child_fd_v1)
        .max()
        .unwrap_or(2);
    let mut next_source_v1 = highest_target_v1.checked_add(1).ok_or(
        ExplicitFdProcessErrorV1::InvalidChildDescriptor {
            child_fd_v1: highest_target_v1,
        },
    )?;
    let cwd_source_v1 = if let Some(working_directory_v1) = request_v1.working_directory_v1 {
        let source_v1 = duplicate_cloexec_at_least_v1(
            working_directory_v1.inner_v1.descriptor_v1.as_fd(),
            next_source_v1,
        )?;
        next_source_v1 = next_source_descriptor_v1(source_v1.as_raw_fd(), highest_target_v1)?;
        Some(source_v1)
    } else {
        None
    };
    let mut extra_sources_v1 = Vec::with_capacity(request_v1.mappings_v1.len());
    for mapping_v1 in &request_v1.mappings_v1 {
        let source_v1 = duplicate_cloexec_at_least_v1(
            ExplicitFdSpawnRequestV1::mapping_source_descriptor_v1(mapping_v1),
            next_source_v1,
        )?;
        next_source_v1 = next_source_descriptor_v1(source_v1.as_raw_fd(), mapping_v1.child_fd_v1)?;
        extra_sources_v1.push(source_v1);
    }
    Ok(SpawnSourcesV1 {
        cwd_v1: cwd_source_v1,
        extras_v1: extra_sources_v1,
    })
}

fn prepare_actions_v1(
    request_v1: &ExplicitFdSpawnRequestV1<'_>,
    stdio_v1: &DarwinStdioFifosV1,
    sources_v1: &SpawnSourcesV1,
) -> Result<SpawnFileActionsV1, ExplicitFdProcessErrorV1> {
    let mut actions_v1 = SpawnFileActionsV1::try_new_v1()?;
    actions_v1.add_open_v1(libc::STDIN_FILENO, stdio_v1.stdin_path_v1(), libc::O_RDONLY)?;
    actions_v1.add_open_v1(libc::STDOUT_FILENO, stdio_v1.stdout_path_v1(), libc::O_WRONLY)?;
    actions_v1.add_open_v1(libc::STDERR_FILENO, stdio_v1.stderr_path_v1(), libc::O_WRONLY)?;
    for (source_v1, mapping_v1) in sources_v1.extras_v1.iter().zip(&request_v1.mappings_v1) {
        actions_v1.add_dup_v1(source_v1.as_raw_fd(), mapping_v1.child_fd_v1)?;
    }
    if let Some(cwd_source_v1) = &sources_v1.cwd_v1 {
        actions_v1.add_fchdir_v1(cwd_source_v1.as_raw_fd())?;
    }
    Ok(actions_v1)
}

fn c_pointer_vector_v1(values_v1: &[CString]) -> Vec<*mut c_char> {
    let mut pointers_v1 = values_v1
        .iter()
        .map(|value_v1| value_v1.as_ptr().cast_mut())
        .collect::<Vec<*mut c_char>>();
    pointers_v1.push(std::ptr::null_mut());
    pointers_v1
}

fn call_posix_spawn_v1(
    launch_image_v1: &DarwinEphemeralLaunchImageV1,
    actions_v1: &SpawnFileActionsV1,
    attributes_v1: &SpawnAttributesV1,
    argv_pointers_v1: &[*mut c_char],
    environment_pointers_v1: &[*mut c_char],
) -> Result<libc::pid_t, ExplicitFdProcessErrorV1> {
    let mut pid_v1 = 0;
    // SAFETY: all C strings/pointer arrays are live for the synchronous call;
    // file actions and attributes were initialized and remain owned by guards.
    let result_v1 = unsafe {
        libc::posix_spawn(
            &mut pid_v1,
            launch_image_v1.executable_path_v1().as_ptr(),
            actions_v1.as_ptr_v1(),
            attributes_v1.as_ptr_v1(),
            argv_pointers_v1.as_ptr(),
            environment_pointers_v1.as_ptr(),
        )
    };
    if result_v1 != 0 {
        return Err(ExplicitFdProcessErrorV1::Spawn {
            operation_v1: "posix_spawn_v1",
            errno_v1: result_v1,
        });
    }
    Ok(pid_v1)
}

fn next_source_descriptor_v1(
    descriptor_v1: i32,
    child_fd_v1: i32,
) -> Result<i32, ExplicitFdProcessErrorV1> {
    descriptor_v1
        .checked_add(1)
        .ok_or(ExplicitFdProcessErrorV1::InvalidChildDescriptor { child_fd_v1 })
}

struct SpawnFileActionsV1 {
    actions_v1: libc::posix_spawn_file_actions_t,
}

impl SpawnFileActionsV1 {
    fn try_new_v1() -> Result<Self, ExplicitFdProcessErrorV1> {
        let mut actions_v1 = std::ptr::null_mut();
        // SAFETY: posix_spawn initializes the opaque actions pointer.
        let result_v1 = unsafe { libc::posix_spawn_file_actions_init(&mut actions_v1) };
        if result_v1 != 0 {
            return Err(ExplicitFdProcessErrorV1::Spawn {
                operation_v1: "posix_spawn_file_actions_init_v1",
                errno_v1: result_v1,
            });
        }
        Ok(Self { actions_v1 })
    }

    fn add_open_v1(
        &mut self,
        child_fd_v1: i32,
        path_v1: &std::ffi::CStr,
        flags_v1: i32,
    ) -> Result<(), ExplicitFdProcessErrorV1> {
        // SAFETY: actions is initialized and path is NUL-terminated.
        let result_v1 = unsafe {
            libc::posix_spawn_file_actions_addopen(
                self.as_mut_ptr_v1(),
                child_fd_v1,
                path_v1.as_ptr(),
                flags_v1 | libc::O_NOFOLLOW,
                0,
            )
        };
        spawn_call_result_v1(result_v1, "posix_spawn_file_actions_addopen_v1")
    }

    fn add_dup_v1(
        &mut self,
        source_v1: i32,
        destination_v1: i32,
    ) -> Result<(), ExplicitFdProcessErrorV1> {
        // SAFETY: actions is initialized and descriptors were validated.
        let result_v1 = unsafe {
            libc::posix_spawn_file_actions_adddup2(self.as_mut_ptr_v1(), source_v1, destination_v1)
        };
        spawn_call_result_v1(result_v1, "posix_spawn_file_actions_adddup2_v1")
    }

    fn add_fchdir_v1(&mut self, directory_v1: i32) -> Result<(), ExplicitFdProcessErrorV1> {
        static SYMBOL_V1: &[u8] = b"posix_spawn_file_actions_addfchdir_np\0";
        // SAFETY: dlsym reads the process symbol table for a static name.
        let symbol_v1 = unsafe { libc::dlsym(libc::RTLD_DEFAULT, SYMBOL_V1.as_ptr().cast()) };
        if symbol_v1.is_null() {
            return Err(ExplicitFdProcessErrorV1::Spawn {
                operation_v1: "posix_spawn_file_actions_addfchdir_np_unavailable_v1",
                errno_v1: libc::ENOTSUP,
            });
        }
        type AddFchdirV1 =
            unsafe extern "C" fn(*mut libc::posix_spawn_file_actions_t, libc::c_int) -> libc::c_int;
        // SAFETY: dlsym returned the named Darwin function; the signature is
        // declared by <spawn.h> and matches AddFchdirV1.
        let add_fchdir_v1: AddFchdirV1 =
            unsafe { std::mem::transmute::<*mut c_void, AddFchdirV1>(symbol_v1) };
        // SAFETY: actions is initialized and directory is a live pinned FD.
        let result_v1 = unsafe { add_fchdir_v1(self.as_mut_ptr_v1(), directory_v1) };
        spawn_call_result_v1(result_v1, "posix_spawn_file_actions_addfchdir_np_v1")
    }

    fn as_ptr_v1(&self) -> *const libc::posix_spawn_file_actions_t {
        &self.actions_v1
    }

    fn as_mut_ptr_v1(&mut self) -> *mut libc::posix_spawn_file_actions_t {
        &mut self.actions_v1
    }
}

impl Drop for SpawnFileActionsV1 {
    fn drop(&mut self) {
        // BEST-EFFORT: opaque action destruction cannot be reported from Drop.
        // SAFETY: actions_v1 remains initialized and uniquely owned by this guard.
        unsafe { libc::posix_spawn_file_actions_destroy(&mut self.actions_v1) };
    }
}

struct SpawnAttributesV1 {
    attributes_v1: libc::posix_spawnattr_t,
}

impl SpawnAttributesV1 {
    fn try_new_v1(isolated_process_group_v1: bool) -> Result<Self, ExplicitFdProcessErrorV1> {
        let mut attributes_v1 = std::ptr::null_mut();
        // SAFETY: posix_spawn initializes the opaque attributes pointer.
        let init_v1 = unsafe { libc::posix_spawnattr_init(&mut attributes_v1) };
        spawn_call_result_v1(init_v1, "posix_spawnattr_init_v1")?;
        let mut owner_v1 = Self { attributes_v1 };
        let raw_flags_v1 = libc::POSIX_SPAWN_CLOEXEC_DEFAULT
            | if isolated_process_group_v1 {
                libc::POSIX_SPAWN_SETPGROUP
            } else {
                0
            };
        let flags_v1 =
            i16::try_from(raw_flags_v1).map_err(|_| ExplicitFdProcessErrorV1::Spawn {
                operation_v1: "posix_spawn_cloexec_flag_range_v1",
                errno_v1: libc::EINVAL,
            })?;
        // SAFETY: attributes is initialized and flags fits c_short.
        let set_v1 =
            unsafe { libc::posix_spawnattr_setflags(&mut owner_v1.attributes_v1, flags_v1) };
        if set_v1 != 0 {
            return Err(ExplicitFdProcessErrorV1::Spawn {
                operation_v1: "posix_spawnattr_setflags_v1",
                errno_v1: set_v1,
            });
        }
        if isolated_process_group_v1 {
            // A pgroup of zero asks posix_spawn to use the child's PID.
            // SAFETY: attributes remains initialized and uniquely owned.
            let set_group_v1 =
                unsafe { libc::posix_spawnattr_setpgroup(&mut owner_v1.attributes_v1, 0) };
            if set_group_v1 != 0 {
                return Err(ExplicitFdProcessErrorV1::Spawn {
                    operation_v1: "posix_spawnattr_setpgroup_v1",
                    errno_v1: set_group_v1,
                });
            }
        }
        Ok(owner_v1)
    }

    fn as_ptr_v1(&self) -> *const libc::posix_spawnattr_t {
        &self.attributes_v1
    }
}

impl Drop for SpawnAttributesV1 {
    fn drop(&mut self) {
        // BEST-EFFORT: opaque attribute destruction cannot be reported from Drop.
        // SAFETY: attributes_v1 remains initialized and uniquely owned by this guard.
        unsafe { libc::posix_spawnattr_destroy(&mut self.attributes_v1) };
    }
}

fn spawn_call_result_v1(
    result_v1: i32,
    operation_v1: &'static str,
) -> Result<(), ExplicitFdProcessErrorV1> {
    if result_v1 != 0 {
        return Err(ExplicitFdProcessErrorV1::Spawn {
            operation_v1,
            errno_v1: result_v1,
        });
    }
    Ok(())
}
