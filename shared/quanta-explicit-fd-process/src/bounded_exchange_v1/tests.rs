//! Transport and Linux kernel-boundary fixtures. These issue no Product DTO.
use super::*;
#[cfg(target_os = "linux")]
use crate::{
    spawn_borrowed_argv_linux_hard_limited_v1, BorrowedEnvironmentV1, ExplicitFdProcessErrorV1,
    PinnedExecutableV1, PinnedWorkingDirectoryV1,
};
use std::cell::Cell;
#[cfg(target_os = "linux")]
use std::ffi::CString;
use std::process::{Child, Command, Stdio};

struct FixtureChildV1(Child);
impl BoundedExchangeChildV1 for FixtureChildV1 {
    fn take_stdin_v1(&mut self) -> Option<File> {
        self.0
            .stdin
            .take()
            .map(|pipe| File::from(std::os::fd::OwnedFd::from(pipe)))
    }
    fn take_stdout_v1(&mut self) -> Option<File> {
        self.0
            .stdout
            .take()
            .map(|pipe| File::from(std::os::fd::OwnedFd::from(pipe)))
    }
    fn take_stderr_v1(&mut self) -> Option<File> {
        self.0
            .stderr
            .take()
            .map(|pipe| File::from(std::os::fd::OwnedFd::from(pipe)))
    }
    fn try_wait_v1(&mut self) -> Result<Option<ExitStatus>, BoundedExchangePollErrorV1> {
        self.0.try_wait().map_err(|_| BoundedExchangePollErrorV1)
    }
}
impl Drop for FixtureChildV1 {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}
fn fixture_v1(script: &str) -> FixtureChildV1 {
    FixtureChildV1(
        Command::new("/bin/sh")
            .args(["-c", script])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap(),
    )
}

#[test]
fn same_poll_owner_preserves_input_parts_exact_output_and_closed_pipe_roles() {
    let mut output = [0; 18];
    let pointer = output.as_ptr();
    assert_eq!(
        poll_bounded_exchange_v1(
            fixture_v1("printf ok >&2; cat"),
            &[b"header", b"source", b"", b"suffix"],
            &mut output,
            || Ok::<_, u8>(()),
            Duration::from_secs(2),
            4096,
        ),
        Ok(18)
    );
    assert_eq!(&output, b"headersourcesuffix");
    assert_eq!(output.as_ptr(), pointer);
    let mut one = [0; 1];
    assert_eq!(
        poll_bounded_exchange_v1(
            fixture_v1("dd if=/dev/zero bs=4096 count=1 1>&2 2>/dev/null; printf x"),
            &[],
            &mut one,
            || Ok::<_, u8>(()),
            Duration::from_secs(2),
            4096,
        ),
        Ok(1)
    );
    assert_eq!(&one, b"x");
}

#[test]
fn distinct_output_stderr_exit_and_wall_failures_never_return_a_frame() {
    for (script, input, wall, expected) in [
        (
            "cat",
            &b"x"[..],
            Duration::from_secs(2),
            BoundedExchangeErrorV1::OutputLimit,
        ),
        (
            "dd if=/dev/zero bs=4097 count=1 1>&2 2>/dev/null",
            &b""[..],
            Duration::from_secs(2),
            BoundedExchangeErrorV1::StderrLimit,
        ),
        (
            "exit 17",
            &b""[..],
            Duration::from_secs(2),
            BoundedExchangeErrorV1::ChildFailed,
        ),
        (
            "while :; do :; done",
            &b""[..],
            Duration::from_millis(5),
            BoundedExchangeErrorV1::WallLimit,
        ),
    ] {
        assert_eq!(
            poll_bounded_exchange_v1(
                fixture_v1(script),
                &[input],
                &mut [],
                || Ok::<_, u8>(()),
                wall,
                4096,
            ),
            Err(expected),
            "{script}"
        );
    }
}

#[test]
fn collected_nonzero_exit_preserves_bounded_stdout_and_actual_status() {
    let mut output = [0; 5];
    let outcome = poll_bounded_exchange_outcome_v1(
        fixture_v1("printf unborn; printf diagnostic >&2; exit 1"),
        &[],
        &mut output,
        || Ok::<_, u8>(()),
        Duration::from_secs(2),
        4096,
        BoundedExchangeExitPolicyV1::CollectStatus,
    );
    // The output bound still wins over a caller-accepted nonzero exit.
    assert_eq!(outcome, Err(BoundedExchangeErrorV1::OutputLimit));
    let mut output = [0; 6];
    let outcome = poll_bounded_exchange_outcome_v1(
        fixture_v1("printf unborn; printf diagnostic >&2; exit 1"),
        &[],
        &mut output,
        || Ok::<_, u8>(()),
        Duration::from_secs(2),
        4096,
        BoundedExchangeExitPolicyV1::CollectStatus,
    )
    .unwrap();
    assert_eq!(outcome.stdout_bytes_v1, 6);
    assert_eq!(&output, b"unborn");
    assert_eq!(outcome.exit_status_v1.code(), Some(1));
    assert!(!outcome.exit_status_v1.success());
}

struct CollectorV1 {
    bytes: Vec<u8>,
    cap: usize,
    appends: usize,
    refusal: Option<(u8, &'static str)>,
}
impl BoundedExchangeOutputV1<(u8, &'static str)> for CollectorV1 {
    fn append_chunk_v1(&mut self, chunk: &[u8]) -> Result<(), (u8, &'static str)> {
        self.appends += 1;
        if let Some(cause) = self.refusal {
            return Err(cause);
        }
        let length = self.bytes.len().checked_add(chunk.len()).unwrap();
        assert!(
            length <= self.cap,
            "poll owner must cap bytes before append"
        );
        self.bytes.try_reserve_exact(chunk.len()).unwrap();
        self.bytes.extend_from_slice(chunk);
        Ok(())
    }
}

#[test]
fn same_poll_collector_uses_actual_output_and_preserves_owner_refusal() {
    let mut output = CollectorV1 {
        bytes: Vec::new(),
        cap: 1 << 20,
        appends: 0,
        refusal: None,
    };
    let result = poll_bounded_exchange_output_v1(
        fixture_v1("printf tiny; exit 1"),
        &[],
        &mut output,
        1 << 20,
        || Ok(()),
        Some(Duration::from_secs(2)),
        4096,
        BoundedExchangeExitPolicyV1::CollectStatus,
    )
    .unwrap();
    assert_eq!(result.stdout_bytes_v1, 4);
    assert_eq!(result.exit_status_v1.code(), Some(1));
    assert_eq!(output.bytes, b"tiny");
    assert_eq!(
        output.bytes.capacity(),
        4,
        "no full installed-cap allocation"
    );
    for cause in [(1, "original-control"), (2, "native-allocation")] {
        let mut output = CollectorV1 {
            bytes: Vec::new(),
            cap: 4,
            appends: 0,
            refusal: Some(cause),
        };
        assert_eq!(
            poll_bounded_exchange_output_v1(
                fixture_v1("printf tiny"),
                &[],
                &mut output,
                4,
                || Ok(()),
                Some(Duration::from_secs(2)),
                4096,
                BoundedExchangeExitPolicyV1::CollectStatus,
            ),
            Err(BoundedExchangeErrorV1::OutputAdmission(cause))
        );
        assert_eq!(output.appends, 1);
        assert_eq!(output.bytes.capacity(), 0);
    }
}

#[test]
fn no_relative_wall_cap_preserves_original_checkpoint_and_reaps_child() {
    let mut output = CollectorV1 {
        bytes: Vec::new(),
        cap: 4,
        appends: 0,
        refusal: None,
    };
    let outcome = poll_bounded_exchange_output_v1(
        fixture_v1("printf tiny"),
        &[],
        &mut output,
        4,
        || Ok(()),
        None,
        4096,
        BoundedExchangeExitPolicyV1::RequireSuccess,
    )
    .unwrap();
    assert!(outcome.exit_status_v1.success());
    assert_eq!(output.bytes, b"tiny");

    let child = fixture_v1("while :; do :; done");
    let pid = child.0.id() as libc::pid_t;
    let calls = Cell::new(0);
    let result = poll_bounded_exchange_output_v1(
        child,
        &[],
        &mut output,
        4,
        || {
            calls.set(calls.get() + 1);
            if calls.get() == 2 {
                Err((37_u8, "original-absolute-deadline"))
            } else {
                Ok(())
            }
        },
        None,
        4096,
        BoundedExchangeExitPolicyV1::RequireSuccess,
    );
    assert_eq!(
        result,
        Err(BoundedExchangeErrorV1::Checkpoint((
            37,
            "original-absolute-deadline"
        )))
    );
    assert_eq!(calls.get(), 2);
    let mut status = 0;
    // SAFETY: this launcher-owned child has just been synchronously reaped.
    assert_eq!(
        unsafe { libc::waitpid(pid, &mut status, libc::WNOHANG) },
        -1
    );
    assert_eq!(
        std::io::Error::last_os_error().raw_os_error(),
        Some(libc::ECHILD)
    );
}

#[test]
fn typed_original_checkpoint_cause_precedes_wall_and_reaps_actual_child() {
    for refuse_at in [1, 2] {
        let child = fixture_v1("while :; do :; done");
        let pid = child.0.id() as libc::pid_t;
        let calls = Cell::new(0);
        let result = poll_bounded_exchange_v1(
            child,
            &[],
            &mut [],
            || {
                calls.set(calls.get() + 1);
                if calls.get() == refuse_at {
                    Err((37_u8, "original-root"))
                } else {
                    Ok(())
                }
            },
            if refuse_at == 1 {
                Duration::ZERO
            } else {
                Duration::from_secs(2)
            },
            4096,
        );
        assert_eq!(
            result,
            Err(BoundedExchangeErrorV1::Checkpoint((37, "original-root")))
        );
        assert_eq!(calls.get(), refuse_at);
        let mut status = 0;
        // SAFETY: this launcher-owned child has just been synchronously reaped.
        assert_eq!(
            unsafe { libc::waitpid(pid, &mut status, libc::WNOHANG) },
            -1
        );
        assert_eq!(
            std::io::Error::last_os_error().raw_os_error(),
            Some(libc::ECHILD)
        );
    }
}

struct ExactFrameV1 {
    bytes: [u8; 4],
    length: usize,
    refusal: Option<(u8, &'static str)>,
}
impl BoundedExchangeOutputV1<(u8, &'static str)> for ExactFrameV1 {
    fn append_chunk_v1(&mut self, chunk: &[u8]) -> Result<(), (u8, &'static str)> {
        if let Some(cause) = self.refusal {
            return Err(cause);
        }
        let end = self.length + chunk.len();
        assert!(end <= self.bytes.len());
        self.bytes[self.length..end].copy_from_slice(chunk);
        self.length = end;
        Ok(())
    }
    fn frame_complete_v1(&self) -> Result<bool, (u8, &'static str)> {
        Ok(self.length == self.bytes.len())
    }
    fn read_limit_v1(&self, remaining: usize) -> Result<usize, (u8, &'static str)> {
        Ok(remaining.min(self.bytes.len() - self.length))
    }
}

#[test]
fn same_poll_session_retains_child_and_pipes_across_exact_frames() {
    let child = fixture_v1("while IFS= read -r line; do printf '%s' \"$line\"; done");
    let pid = child.0.id() as libc::pid_t;
    let mut session =
        BoundedExchangeSessionV1::try_new_v1::<(u8, &'static str)>(child, 4096).unwrap();
    for frame in [b"one1\n", b"two2\n"] {
        let mut output = ExactFrameV1 {
            bytes: [0; 4],
            length: 0,
            refusal: None,
        };
        assert_eq!(
            session.exchange_frame_v1(&[frame], &mut output, 4, || Ok(()), None),
            Ok(4)
        );
        assert_eq!(&output.bytes, &frame[..4]);
        assert!(session
            .child_v1
            .as_mut()
            .unwrap()
            .try_wait_v1()
            .unwrap()
            .is_none());
        assert!(
            session.stdin_v1.is_some(),
            "frame must retain the request pipe"
        );
    }
    drop(session);
    let mut status = 0;
    // SAFETY: this launcher-owned child has just been synchronously reaped.
    assert_eq!(
        unsafe { libc::waitpid(pid, &mut status, libc::WNOHANG) },
        -1
    );
    assert_eq!(
        std::io::Error::last_os_error().raw_os_error(),
        Some(libc::ECHILD)
    );
}

#[test]
fn session_refusal_poison_reaps_before_return_and_keeps_typed_cause() {
    for checkpoint_refusal in [false, true] {
        let child = fixture_v1("while IFS= read -r line; do printf '%s' \"$line\"; done");
        let pid = child.0.id() as libc::pid_t;
        let mut session =
            BoundedExchangeSessionV1::try_new_v1::<(u8, &'static str)>(child, 4096).unwrap();
        let cause = (37_u8, "original-root");
        let mut output = ExactFrameV1 {
            bytes: [0; 4],
            length: 0,
            refusal: (!checkpoint_refusal).then_some(cause),
        };
        let result = session.exchange_frame_v1(
            &[b"one1\n"],
            &mut output,
            4,
            || {
                if checkpoint_refusal {
                    Err(cause)
                } else {
                    Ok(())
                }
            },
            None,
        );
        assert_eq!(
            result,
            Err(if checkpoint_refusal {
                BoundedExchangeErrorV1::Checkpoint(cause)
            } else {
                BoundedExchangeErrorV1::OutputAdmission(cause)
            })
        );
        assert!(session.child_v1.is_none());
        let mut status = 0;
        // SAFETY: failure must reap even though the session remains in scope.
        assert_eq!(
            unsafe { libc::waitpid(pid, &mut status, libc::WNOHANG) },
            -1
        );
        assert_eq!(
            std::io::Error::last_os_error().raw_os_error(),
            Some(libc::ECHILD)
        );
        assert_eq!(
            session.exchange_frame_v1(&[], &mut output, 4, || Ok(()), None),
            Err(BoundedExchangeErrorV1::ChildFailed)
        );
    }
}

struct CreditFrameV1 {
    bytes: [u8; 4],
    length: usize,
    pending_grant: bool,
    result: bool,
    credit_requests: usize,
    granted: usize,
    refuse: bool,
}
impl BoundedExchangeOutputV1<(u8, &'static str)> for CreditFrameV1 {
    fn append_chunk_v1(&mut self, chunk: &[u8]) -> Result<(), (u8, &'static str)> {
        let end = self.length + chunk.len();
        self.bytes[self.length..end].copy_from_slice(chunk);
        self.length = end;
        if self.length == self.bytes.len() {
            if &self.bytes == b"CRED" {
                self.credit_requests += 1;
                self.pending_grant = true;
                self.length = 0;
            } else {
                assert_eq!(&self.bytes, b"DONE");
                self.result = true;
            }
        }
        Ok(())
    }
    fn write_continuation_v1(&mut self, output: &mut [u8]) -> Result<usize, (u8, &'static str)> {
        if !self.pending_grant {
            return Ok(0);
        }
        if self.refuse {
            return Err((37, "original-native-work"));
        }
        self.granted += 1;
        self.pending_grant = false;
        output[..5].copy_from_slice(b"GRNT\n");
        Ok(5)
    }
    fn retained_output_bytes_v1(&self) -> Option<usize> {
        Some(self.length)
    }
    fn frame_complete_v1(&self) -> Result<bool, (u8, &'static str)> {
        Ok(self.result)
    }
    fn read_limit_v1(&self, remaining: usize) -> Result<usize, (u8, &'static str)> {
        Ok(remaining.min(self.bytes.len() - self.length))
    }
}

#[test]
fn same_session_sends_fixed_credit_only_after_owner_grant_and_retains_first_refusal() {
    for refuse in [false, true] {
        let child = fixture_v1(
            "IFS= read -r command; printf CRED; IFS= read -r grant; \
             [ \"$grant\" = GRNT ] || exit 17; printf DONE; \
             while IFS= read -r command; do :; done",
        );
        let pid = child.0.id() as libc::pid_t;
        let mut session =
            BoundedExchangeSessionV1::try_new_v1::<(u8, &'static str)>(child, 4096).unwrap();
        let mut output = CreditFrameV1 {
            bytes: [0; 4],
            length: 0,
            pending_grant: false,
            result: false,
            credit_requests: 0,
            granted: 0,
            refuse,
        };
        let result = session.exchange_frame_v1(&[b"OPEN\n"], &mut output, 4, || Ok(()), None);
        assert_eq!(output.credit_requests, 1);
        if refuse {
            assert_eq!(
                result,
                Err(BoundedExchangeErrorV1::OutputAdmission((
                    37,
                    "original-native-work"
                )))
            );
            assert_eq!(output.granted, 0);
            assert!(
                !output.result,
                "child producer must still wait for its first grant"
            );
            let mut status = 0;
            // SAFETY: refusal must kill/reap before returning this original cause.
            assert_eq!(
                unsafe { libc::waitpid(pid, &mut status, libc::WNOHANG) },
                -1
            );
            assert_eq!(
                std::io::Error::last_os_error().raw_os_error(),
                Some(libc::ECHILD)
            );
        } else {
            assert_eq!(
                result,
                Ok(8),
                "credit bytes released before the result span"
            );
            assert_eq!(output.granted, 1);
            assert!(output.result);
            assert!(session.child_v1.is_some());
        }
    }
}

#[cfg(target_os = "linux")]
fn pinned_test_v1() -> PinnedExecutableV1 {
    use sha2::{Digest, Sha256};
    let path = std::env::current_exe().unwrap();
    let mut input = File::open(&path).unwrap();
    let mut digest = Sha256::new();
    let mut buffer = [0; 8192];
    loop {
        let read = input.read(&mut buffer).unwrap();
        if read == 0 {
            break;
        }
        digest.update(&buffer[..read]);
    }
    PinnedExecutableV1::try_pin_v1(
        File::open(path).unwrap(),
        &format!("{:x}", digest.finalize()),
    )
    .unwrap()
}

#[cfg(target_os = "linux")]
#[test]
fn borrowed_fixed_plan_uses_live_environment_pinned_cwd_and_preexec_hard_limits() {
    use std::os::fd::AsFd;
    let executable = pinned_test_v1();
    let directory = tempfile::tempdir().unwrap();
    let cwd_file = File::open(directory.path()).unwrap();
    let cwd = PinnedWorkingDirectoryV1::try_pin_v1(cwd_file.as_fd()).unwrap();
    let entries = [CString::new("QUANTA_BORROWED_EXCHANGE_PROBE=1").unwrap()];
    let pointers = [entries[0].as_ptr().cast_mut(), std::ptr::null_mut()];
    let environment = BorrowedEnvironmentV1::try_new_v1(&entries, &pointers).unwrap();
    assert!(matches!(
        spawn_borrowed_argv_linux_hard_limited_v1(
            &executable,
            &[],
            environment,
            Some(&cwd),
            1 << 30,
        ),
        Err(ExplicitFdProcessErrorV1::EmptyArgumentVector)
    ));
    assert!(matches!(
        spawn_borrowed_argv_linux_hard_limited_v1(
            &executable,
            &[c"x"; 7],
            environment,
            Some(&cwd),
            1 << 30,
        ),
        Err(ExplicitFdProcessErrorV1::TooManyBorrowedArguments)
    ));
    let child = spawn_borrowed_argv_linux_hard_limited_v1(
        &executable,
        &[
            c"quanta",
            c"--exact",
            c"bounded_exchange_v1::tests::child_borrowed_probe_v1",
            c"--nocapture",
        ],
        environment,
        Some(&cwd),
        1 << 30,
    )
    .unwrap();
    let mut output = [0; 4096];
    let received = poll_bounded_exchange_v1(
        child,
        &[],
        &mut output,
        || Ok::<_, u8>(()),
        Duration::from_secs(5),
        4096,
    )
    .unwrap();
    let text = std::str::from_utf8(&output[..received]).unwrap();
    assert!(text.contains("hard=1073741824:1073741824 data=1073741824:1073741824 denied=true"));
    assert!(text.contains(directory.path().to_str().unwrap()));
    assert!(text.contains("environment=1"));
    // View and actual storage remain live through reap; no captured env copy.
    assert_eq!(pointers[0], entries[0].as_ptr().cast_mut());
}

#[cfg(target_os = "linux")]
#[test]
fn child_borrowed_probe_v1() {
    if std::env::var_os("QUANTA_BORROWED_EXCHANGE_PROBE").is_none() {
        return;
    }
    let mut address = libc::rlimit {
        rlim_cur: 0,
        rlim_max: 0,
    };
    let mut data = address;
    // SAFETY: getrlimit fills stack storage; this fixture owns its child process.
    assert_eq!(unsafe { libc::getrlimit(libc::RLIMIT_AS, &mut address) }, 0);
    // SAFETY: getrlimit writes the data limit into the live stack slot.
    assert_eq!(unsafe { libc::getrlimit(libc::RLIMIT_DATA, &mut data) }, 0);
    // SAFETY: anonymous mapping creates no aliases; a successful mapping is
    // released before this isolated fixture exits.
    let mapping = unsafe {
        libc::mmap(
            std::ptr::null_mut(),
            2 << 30,
            libc::PROT_READ | libc::PROT_WRITE,
            libc::MAP_PRIVATE | libc::MAP_ANONYMOUS,
            -1,
            0,
        )
    };
    let denied = mapping == libc::MAP_FAILED;
    if !denied {
        // SAFETY: a successful mmap returned this exact live mapping.
        unsafe {
            libc::munmap(mapping, 2 << 30);
        }
    }
    assert!(denied);
    println!(
        "hard={}:{} data={}:{} denied={} environment={} cwd={}",
        address.rlim_cur,
        address.rlim_max,
        data.rlim_cur,
        data.rlim_max,
        denied,
        std::env::var("QUANTA_BORROWED_EXCHANGE_PROBE").unwrap(),
        std::env::current_dir().unwrap().display()
    );
}
