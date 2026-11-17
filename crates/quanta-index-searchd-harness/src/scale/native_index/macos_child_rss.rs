//! A single owned child's sampled resident bytes, never physical footprint.
//!
//! `proc_pid_rusage` returns no task port or file descriptor to release. The
//! caller must retain the unreaped `Child` while this sampler is alive. Bind the
//! first usable kernel identity and permanently refuse any later replacement.
//! XNU `fill_task_rusage` reads `ri_resident_size` from `task_ledgers.phys_mem`;
//! `task.c` declares this ledger in bytes. `ri_phys_footprint` is separate.
//! Sources: apple-oss-distributions/xnu, `osfmk/kern/bsd_kern.c` and `task.c`.

use std::sync::atomic::{AtomicBool, Ordering};

use anyhow::{Context as _, Result, ensure};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct ProcessIdentity {
    started: u64,
    executable_uuid: [u8; 16],
}

struct NativeSample {
    identity: ProcessIdentity,
    resident_bytes: u64,
    exited: u64,
}

pub(super) struct MacChildRss {
    pid: u32,
    identity: Option<ProcessIdentity>,
    invalidated: bool,
}

impl MacChildRss {
    pub(super) const fn new(pid: u32) -> Self {
        Self {
            pid,
            identity: None,
            invalidated: false,
        }
    }

    #[cfg(target_os = "macos")]
    pub(super) fn sample(&mut self, stopped: &AtomicBool) -> Result<u64> {
        self.sample_with(stopped, |pid| {
            use libproc::pid_rusage::{RUsageInfoV0, pidrusage};
            let sample = pidrusage::<RUsageInfoV0>(pid)
                .map_err(|error| anyhow::anyhow!("child proc_pid_rusage: {error}"))?;
            Ok(NativeSample::from_usage(&sample))
        })
    }

    fn sample_with(
        &mut self,
        stopped: &AtomicBool,
        read: impl FnOnce(i32) -> Result<NativeSample>,
    ) -> Result<u64> {
        ensure!(!self.invalidated, "child RSS identity was replaced");
        ensure!(
            !stopped.load(Ordering::Acquire),
            "child RSS probe cancelled"
        );
        let pid = i32::try_from(self.pid).context("child RSS PID exceeds native range")?;
        ensure!(pid > 0, "child RSS PID is not positive");
        let sample = read(pid)?;
        ensure!(
            !stopped.load(Ordering::Acquire),
            "child RSS probe cancelled"
        );
        ensure!(
            sample.identity.started != 0,
            "child RSS start identity missing"
        );
        match self.identity {
            Some(original) if sample.identity != original => {
                self.invalidated = true;
                anyhow::bail!("child RSS process identity changed");
            }
            Some(_) => {}
            None => self.identity = Some(sample.identity),
        }
        ensure!(sample.exited == 0, "child RSS process already exited");
        ensure!(sample.resident_bytes != 0, "child RSS unavailable or zero");
        Ok(sample.resident_bytes)
    }
}

#[cfg(target_os = "macos")]
impl NativeSample {
    fn from_usage(sample: &libproc::pid_rusage::RUsageInfoV0) -> Self {
        Self {
            identity: ProcessIdentity {
                started: sample.ri_proc_start_abstime,
                executable_uuid: sample.ri_uuid,
            },
            resident_bytes: sample.ri_resident_size,
            exited: sample.ri_proc_exit_abstime,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use anyhow::anyhow;

    fn row(started: u64, resident_bytes: u64) -> NativeSample {
        NativeSample {
            identity: ProcessIdentity {
                started,
                executable_uuid: [7; 16],
            },
            resident_bytes,
            exited: 0,
        }
    }

    #[test]
    fn resident_bytes_are_not_scaled_and_may_decrease() -> Result<()> {
        let mut sampler = MacChildRss::new(42);
        let stopped = AtomicBool::new(false);
        ensure!(sampler.sample_with(&stopped, |_| Ok(row(10, 8193)))? == 8193);
        ensure!(sampler.sample_with(&stopped, |_| Ok(row(10, 1025)))? == 1025);
        Ok(())
    }

    #[test]
    fn identity_replacement_is_permanent_and_cannot_rebind() -> Result<()> {
        let mut sampler = MacChildRss::new(42);
        let stopped = AtomicBool::new(false);
        ensure!(sampler.sample_with(&stopped, |_| Ok(row(10, 8193)))? == 8193);
        ensure!(
            sampler
                .sample_with(&stopped, |_| Ok(row(11, 8193)))
                .is_err()
        );
        let mut called = false;
        ensure!(
            sampler
                .sample_with(&stopped, |_| {
                    called = true;
                    Ok(row(10, 8193))
                })
                .is_err()
        );
        ensure!(!called);
        Ok(())
    }

    #[test]
    fn syscall_failure_missing_zero_exit_and_cancel_are_missing_samples() -> Result<()> {
        let stopped = AtomicBool::new(false);
        let mut sampler = MacChildRss::new(42);
        ensure!(
            sampler
                .sample_with(&stopped, |_| Err(anyhow!("EPERM")))
                .is_err()
        );
        ensure!(sampler.identity.is_none());
        ensure!(sampler.sample_with(&stopped, |_| Ok(row(0, 8193))).is_err());
        ensure!(sampler.identity.is_none());
        ensure!(sampler.sample_with(&stopped, |_| Ok(row(10, 0))).is_err());
        ensure!(sampler.identity.is_some());
        let mut exited = row(10, 8193);
        exited.exited = 99;
        ensure!(sampler.sample_with(&stopped, |_| Ok(exited)).is_err());
        ensure!(
            sampler
                .sample_with(&stopped, |_| {
                    stopped.store(true, Ordering::Release);
                    Ok(row(10, 8193))
                })
                .is_err()
        );
        ensure!(
            sampler
                .sample_with(&stopped, |_| Err(anyhow!("must not call")))
                .is_err()
        );
        Ok(())
    }

    #[test]
    fn invalid_pid_is_rejected_before_calling_native_probe() {
        let stopped = AtomicBool::new(false);
        for pid in [0, u32::MAX] {
            let mut called = false;
            let result = MacChildRss::new(pid).sample_with(&stopped, |_| {
                called = true;
                Ok(row(10, 8193))
            });
            assert!(result.is_err());
            assert!(!called);
        }
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn native_backend_reads_current_process_without_a_subprocess() -> Result<()> {
        let mut sampler = MacChildRss::new(std::process::id());
        let stopped = AtomicBool::new(false);
        ensure!(sampler.sample(&stopped)? > 0);
        ensure!(sampler.sample(&stopped)? > 0);
        ensure!(sampler.identity.is_some());
        Ok(())
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn native_adapter_selects_resident_bytes_not_footprint() {
        let sample = libproc::pid_rusage::RUsageInfoV0 {
            ri_resident_size: 8193,
            ri_phys_footprint: 999_999,
            ri_proc_start_abstime: 10,
            ..libproc::pid_rusage::RUsageInfoV0::default()
        };
        let mapped = NativeSample::from_usage(&sample);
        assert_eq!(mapped.resident_bytes, 8193);
        assert_eq!(mapped.identity.started, 10);
    }
}
