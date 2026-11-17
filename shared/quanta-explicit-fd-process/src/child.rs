use crate::{ExplicitFdProcessErrorV1, SpawnedProcessV1};
use std::fs::File;
use std::process::ExitStatus;

#[derive(Debug)]
pub struct ExplicitFdChildV1 {
    pid_v1: libc::pid_t,
    isolated_process_group_v1: bool,
    stdin_v1: Option<File>,
    stdout_v1: Option<File>,
    stderr_v1: Option<File>,
    process_lifetime_guard_v1: Option<crate::platform::ProcessLifetimeGuardV1>,
    isolated_process_group_killed_v1: bool,
    waited_v1: bool,
}

impl ExplicitFdChildV1 {
    pub(crate) fn from_spawned_v1(
        spawned_v1: Result<SpawnedProcessV1, ExplicitFdProcessErrorV1>,
    ) -> Result<Self, ExplicitFdProcessErrorV1> {
        let spawned_v1 = spawned_v1?;
        Ok(Self {
            pid_v1: spawned_v1.pid_v1,
            isolated_process_group_v1: spawned_v1.isolated_process_group_v1,
            stdin_v1: Some(File::from(spawned_v1.stdin_v1)),
            stdout_v1: Some(File::from(spawned_v1.stdout_v1)),
            stderr_v1: Some(File::from(spawned_v1.stderr_v1)),
            process_lifetime_guard_v1: spawned_v1.process_lifetime_guard_v1,
            isolated_process_group_killed_v1: false,
            waited_v1: false,
        })
    }

    #[must_use]
    pub const fn pid_v1(&self) -> libc::pid_t {
        self.pid_v1
    }

    pub fn take_stdin_v1(&mut self) -> Option<File> {
        self.stdin_v1.take()
    }

    pub fn take_stdout_v1(&mut self) -> Option<File> {
        self.stdout_v1.take()
    }

    pub fn take_stderr_v1(&mut self) -> Option<File> {
        self.stderr_v1.take()
    }

    pub fn wait_v1(&mut self) -> Result<ExitStatus, ExplicitFdProcessErrorV1> {
        if self.waited_v1 {
            return Err(ExplicitFdProcessErrorV1::ChildAlreadyWaited);
        }
        if self.isolated_process_group_v1 {
            crate::platform::observe_pid_exit_without_reap_v1(self.pid_v1, false)?;
            self.kill_isolated_process_group_after_leader_exit_once_v1()?;
        }
        let status_v1 = crate::platform::wait_pid_v1(self.pid_v1)?;
        self.waited_v1 = true;
        self.release_process_lifetime_guard_v1();
        Ok(status_v1)
    }

    /// Poll child completion without embedding a timeout policy in the launcher.
    pub fn try_wait_v1(&mut self) -> Result<Option<ExitStatus>, ExplicitFdProcessErrorV1> {
        if self.waited_v1 {
            return Err(ExplicitFdProcessErrorV1::ChildAlreadyWaited);
        }
        let status_v1 = if self.isolated_process_group_v1 {
            if !crate::platform::observe_pid_exit_without_reap_v1(self.pid_v1, true)? {
                return Ok(None);
            }
            self.kill_isolated_process_group_after_leader_exit_once_v1()?;
            Some(crate::platform::wait_pid_v1(self.pid_v1)?)
        } else {
            crate::platform::try_wait_pid_v1(self.pid_v1)?
        };
        if status_v1.is_some() {
            self.waited_v1 = true;
            self.release_process_lifetime_guard_v1();
        }
        Ok(status_v1)
    }

    pub fn kill_v1(&mut self) -> Result<(), ExplicitFdProcessErrorV1> {
        if self.waited_v1 {
            return Err(ExplicitFdProcessErrorV1::ChildAlreadyWaited);
        }
        crate::platform::kill_process_v1(self.pid_v1, self.isolated_process_group_v1)?;
        if self.isolated_process_group_v1 {
            self.isolated_process_group_killed_v1 = true;
        }
        Ok(())
    }

    fn kill_isolated_process_group_after_leader_exit_once_v1(
        &mut self,
    ) -> Result<(), ExplicitFdProcessErrorV1> {
        if self.isolated_process_group_killed_v1 {
            return Ok(());
        }
        crate::platform::kill_process_group_after_leader_exit_v1(self.pid_v1)?;
        self.isolated_process_group_killed_v1 = true;
        Ok(())
    }

    fn release_process_lifetime_guard_v1(&mut self) {
        let _guard_v1 = self.process_lifetime_guard_v1.take();
    }
}

impl Drop for ExplicitFdChildV1 {
    fn drop(&mut self) {
        if !self.waited_v1 {
            // BEST-EFFORT: Drop cannot return an error; killing and reaping the
            // owned PID is safer than leaving a running child or zombie.
            crate::platform::terminate_and_reap_v1(self.pid_v1, self.isolated_process_group_v1);
            self.waited_v1 = true;
        }
        self.release_process_lifetime_guard_v1();
    }
}
