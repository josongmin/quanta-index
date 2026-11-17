//! Deny-by-default syscall closure for the retained static Z3 image.
//!
//! The launcher retains only stdin/stdout/stderr after exec. No path-opening,
//! networking, process creation, namespace, tracing, or io_uring syscall is
//! admitted. An unsupported ABI or attempted syscall kills the solver process.

const AUDIT_ARCH_X86_64_V1: u32 = 0xc000_003e;
const X32_SYSCALL_BIT_V1: u32 = 0x4000_0000;
const RET_KILL_PROCESS_V1: u32 = 0x8000_0000;
const RET_ALLOW_V1: u32 = 0x7fff_0000;

const fn statement_v1(code_v1: u16, value_v1: u32) -> libc::sock_filter {
    libc::sock_filter {
        code: code_v1,
        jt: 0,
        jf: 0,
        k: value_v1,
    }
}

const fn branch_v1(code_v1: u16, value_v1: u32, yes_v1: u8, no_v1: u8) -> libc::sock_filter {
    libc::sock_filter {
        code: code_v1,
        jt: yes_v1,
        jf: no_v1,
        k: value_v1,
    }
}

/// BPF loads seccomp_data.arch at offset 4 and nr at offset 0. The x32 bit
/// must be refused separately because x86-64 and x32 share the audit arch.
pub(super) fn filter_v1(executable_fd_v1: libc::c_int) -> Vec<libc::sock_filter> {
    // seccomp_data.args[0] and args[4] are the image descriptor and flags of
    // execveat. Pin both to the parent-retained image and AT_EMPTY_PATH.
    assert!(executable_fd_v1 >= 0);
    let allowed_v1: &[libc::c_long] = &[
        libc::SYS_read,
        libc::SYS_write,
        libc::SYS_readv,
        libc::SYS_writev,
        libc::SYS_close,
        libc::SYS_fstat,
        libc::SYS_lseek,
        libc::SYS_brk,
        libc::SYS_mmap,
        libc::SYS_mprotect,
        libc::SYS_munmap,
        libc::SYS_mremap,
        libc::SYS_madvise,
        libc::SYS_arch_prctl,
        libc::SYS_set_tid_address,
        libc::SYS_set_robust_list,
        libc::SYS_rseq,
        libc::SYS_rt_sigaction,
        libc::SYS_rt_sigprocmask,
        libc::SYS_rt_sigreturn,
        libc::SYS_sigaltstack,
        libc::SYS_futex,
        libc::SYS_getpid,
        libc::SYS_gettid,
        libc::SYS_clock_gettime,
        libc::SYS_clock_getres,
        libc::SYS_nanosleep,
        libc::SYS_clock_nanosleep,
        libc::SYS_sched_yield,
        libc::SYS_getrusage,
        libc::SYS_exit,
        libc::SYS_exit_group,
    ];
    let mut filter_v1 = Vec::with_capacity(20 + 2 * allowed_v1.len());
    filter_v1.push(statement_v1(0x20, 4));
    filter_v1.push(branch_v1(0x15, AUDIT_ARCH_X86_64_V1, 1, 0));
    filter_v1.push(statement_v1(0x06, RET_KILL_PROCESS_V1));
    filter_v1.push(statement_v1(0x20, 0));
    filter_v1.push(branch_v1(0x45, X32_SYSCALL_BIT_V1, 0, 1));
    filter_v1.push(statement_v1(0x06, RET_KILL_PROCESS_V1));
    for syscall_v1 in allowed_v1 {
        filter_v1.push(branch_v1(0x15, *syscall_v1 as u32, 0, 1));
        filter_v1.push(statement_v1(0x06, RET_ALLOW_V1));
    }
    filter_v1.push(branch_v1(0x15, libc::SYS_execveat as u32, 1, 0));
    filter_v1.push(statement_v1(0x06, RET_KILL_PROCESS_V1));
    filter_v1.push(statement_v1(0x20, 16));
    filter_v1.push(branch_v1(0x15, executable_fd_v1 as u32, 1, 0));
    filter_v1.push(statement_v1(0x06, RET_KILL_PROCESS_V1));
    filter_v1.push(statement_v1(0x20, 20));
    filter_v1.push(branch_v1(0x15, 0, 1, 0));
    filter_v1.push(statement_v1(0x06, RET_KILL_PROCESS_V1));
    filter_v1.push(statement_v1(0x20, 48));
    filter_v1.push(branch_v1(0x15, libc::AT_EMPTY_PATH as u32, 1, 0));
    filter_v1.push(statement_v1(0x06, RET_KILL_PROCESS_V1));
    filter_v1.push(statement_v1(0x20, 52));
    filter_v1.push(branch_v1(0x15, 0, 1, 0));
    filter_v1.push(statement_v1(0x06, RET_KILL_PROCESS_V1));
    filter_v1.push(statement_v1(0x06, RET_ALLOW_V1));
    filter_v1.push(statement_v1(0x06, RET_KILL_PROCESS_V1));
    filter_v1
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{spawn_confined_z3_linux_v1, PinnedExecutableV1, PinnedWorkingDirectoryV1};
    use sha2::{Digest, Sha256};
    use std::fs::File;
    use std::os::unix::fs::PermissionsExt;
    use std::os::unix::process::ExitStatusExt;

    #[test]
    fn policy_rejects_ambient_and_cross_abi_syscalls_v1() {
        let filter_v1 = filter_v1(23);
        assert_eq!(filter_v1[0].k, 4);
        assert_eq!(filter_v1[1].k, AUDIT_ARCH_X86_64_V1);
        assert_eq!(filter_v1[4].k, X32_SYSCALL_BIT_V1);
        let admitted_v1: Vec<u32> = filter_v1[6..filter_v1.len() - 16]
            .chunks_exact(2)
            .map(|pair_v1| pair_v1[0].k)
            .collect();
        for refused_v1 in [
            libc::SYS_open,
            libc::SYS_openat,
            libc::SYS_socket,
            libc::SYS_connect,
            libc::SYS_clone,
            libc::SYS_fork,
            libc::SYS_ptrace,
            libc::SYS_io_uring_setup,
        ] {
            assert!(!admitted_v1.contains(&(refused_v1 as u32)));
        }
        assert!(!admitted_v1.contains(&(libc::SYS_execveat as u32)));
        assert!(!admitted_v1.contains(&(libc::SYS_tgkill as u32)));
        assert!(!admitted_v1.contains(&(libc::SYS_prlimit64 as u32)));
        assert_eq!(filter_v1[filter_v1.len() - 16].k, libc::SYS_execveat as u32);
        assert_eq!(filter_v1[filter_v1.len() - 13].k, 23);
    }

    fn static_probe_image_v1(code_v1: &[u8]) -> Vec<u8> {
        let mut image_v1 = vec![0u8; 120];
        image_v1[..7].copy_from_slice(b"\x7fELF\x02\x01\x01");
        image_v1[16..18].copy_from_slice(&2u16.to_le_bytes());
        image_v1[18..20].copy_from_slice(&62u16.to_le_bytes());
        image_v1[20..24].copy_from_slice(&1u32.to_le_bytes());
        image_v1[24..32].copy_from_slice(&0x400078u64.to_le_bytes());
        image_v1[32..40].copy_from_slice(&64u64.to_le_bytes());
        image_v1[52..54].copy_from_slice(&64u16.to_le_bytes());
        image_v1[54..56].copy_from_slice(&56u16.to_le_bytes());
        image_v1[56..58].copy_from_slice(&1u16.to_le_bytes());
        image_v1[64..68].copy_from_slice(&1u32.to_le_bytes());
        image_v1[68..72].copy_from_slice(&5u32.to_le_bytes());
        image_v1[80..88].copy_from_slice(&0x400000u64.to_le_bytes());
        image_v1[96..104].copy_from_slice(&(120u64 + code_v1.len() as u64).to_le_bytes());
        image_v1[104..112].copy_from_slice(&(120u64 + code_v1.len() as u64).to_le_bytes());
        image_v1.extend_from_slice(code_v1);
        image_v1
    }

    #[test]
    fn confined_static_image_executes_but_openat_is_killed_v1() {
        let directory_v1 = tempfile::tempdir().expect("fixture directory");
        let cwd_v1 = PinnedWorkingDirectoryV1::try_pin_v1(
            File::open(directory_v1.path()).expect("fixture cwd"),
        )
        .expect("pin fixture cwd");
        for (name_v1, code_v1, allowed_v1) in [
            ("exit", &b"\xb8\xe7\x00\x00\x00\x31\xff\x0f\x05"[..], true),
            (
                "openat",
                &b"\xb8\x01\x01\x00\x00\x31\xff\x31\xf6\x31\xd2\x0f\x05\xb8\xe7\x00\x00\x00\x31\xff\x0f\x05"[..],
                false,
            ),
            (
                "foreign_exec_fd",
                &b"\xb8\x42\x01\x00\x00\x31\xff\x31\xf6\x31\xd2\x45\x31\xd2\x41\xb8\x00\x10\x00\x00\x0f\x05\xb8\xe7\x00\x00\x00\x31\xff\x0f\x05"[..],
                false,
            ),
        ] {
            let path_v1 = directory_v1.path().join(name_v1);
            let image_v1 = static_probe_image_v1(code_v1);
            std::fs::write(&path_v1, &image_v1).expect("write static probe");
            std::fs::set_permissions(&path_v1, std::fs::Permissions::from_mode(0o500))
                .expect("make probe executable");
            let source_v1 = File::open(&path_v1).expect("open probe read only");
            crate::verify_static_linux_x86_64_elf_v1(&source_v1)
                .expect("probe uses the supported static ELF closure");
            let digest_v1 = format!("{:x}", Sha256::digest(&image_v1));
            let executable_v1 =
                PinnedExecutableV1::try_pin_v1(&source_v1, &digest_v1).expect("pin probe");
            let mut child_v1 = spawn_confined_z3_linux_v1(
                &executable_v1,
                &[c"probe"],
                &cwd_v1,
                64 * 1024 * 1024,
            )
            .expect("spawn confined probe");
            let status_v1 = child_v1.wait_v1().expect("reap probe");
            if allowed_v1 {
                assert!(status_v1.success());
            } else {
                assert_eq!(status_v1.signal(), Some(libc::SIGSYS));
            }
        }
    }
}
