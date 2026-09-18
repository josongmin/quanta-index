//! The daemon's own resident memory, from the kernel (QI-BB-016).
//!
//! Linux reports the current resident set in `/proc/self/status`
//! (`VmRSS`), which is what the gauge and the writer gate read. The BSDs
//! and Darwin expose only the process high-water mark through
//! `getrusage(RUSAGE_SELF)`, so there the reading is the peak resident
//! set since the process started — a conservative pressure signal (it
//! never under-reports, and once above a ceiling it stays there until
//! restart), which the probe names in its error text and the boot log
//! names at start.

use quanta_index_core::{CoreError, ProcessMemoryProbePort};

/// The kernel's accounting of this process's resident memory.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct KernelResidentMemoryProbe;

impl KernelResidentMemoryProbe {
    /// What this platform's reading means: the current resident set, or
    /// its high-water mark where the kernel reports only that.
    #[must_use]
    pub const fn semantics() -> &'static str {
        if cfg!(target_os = "linux") {
            "current resident set (VmRSS)"
        } else {
            "peak resident set since process start (ru_maxrss); the current set is not reported on this platform"
        }
    }
}

impl ProcessMemoryProbePort for KernelResidentMemoryProbe {
    fn resident_bytes(&self) -> Result<u64, CoreError> {
        kernel_resident_bytes()
    }
}

#[cfg(target_os = "linux")]
fn kernel_resident_bytes() -> Result<u64, CoreError> {
    let status = std::fs::read_to_string("/proc/self/status").map_err(|error| {
        CoreError::Storage(format!("process memory: read /proc/self/status: {error}"))
    })?;
    parse_vm_rss_bytes(&status)
}

/// The `VmRSS:` line of `/proc/self/status`, in bytes.
#[cfg(any(target_os = "linux", test))]
fn parse_vm_rss_bytes(status: &str) -> Result<u64, CoreError> {
    let line = status
        .lines()
        .find_map(|line| line.strip_prefix("VmRSS:"))
        .ok_or_else(|| {
            CoreError::Storage("process memory: /proc/self/status has no VmRSS line".to_string())
        })?;
    let mut fields = line.split_whitespace();
    let value = fields
        .next()
        .ok_or_else(|| {
            CoreError::Storage(format!("process memory: VmRSS line has no value: `{line}`"))
        })?
        .parse::<u64>()
        .map_err(|error| {
            CoreError::Storage(format!(
                "process memory: VmRSS value is not a count: `{line}`: {error}"
            ))
        })?;
    let unit = fields.next().ok_or_else(|| {
        CoreError::Storage(format!("process memory: VmRSS line has no unit: `{line}`"))
    })?;
    let scale = match unit {
        "kB" => 1024_u64,
        "mB" => 1024_u64 * 1024,
        "B" => 1,
        other => {
            return Err(CoreError::Storage(format!(
                "process memory: VmRSS unit `{other}` is not one this probe reads"
            )));
        }
    };
    Ok(value.saturating_mul(scale))
}

#[cfg(not(target_os = "linux"))]
fn kernel_resident_bytes() -> Result<u64, CoreError> {
    use nix::sys::resource::{UsageWho, getrusage};
    let usage = getrusage(UsageWho::RUSAGE_SELF).map_err(|error| {
        CoreError::Storage(format!("process memory: getrusage(RUSAGE_SELF): {error}"))
    })?;
    let max_rss = u64::try_from(usage.max_rss()).map_err(|error| {
        CoreError::Storage(format!(
            "process memory: ru_maxrss {} is not a byte count: {error}",
            usage.max_rss()
        ))
    })?;
    // Darwin reports bytes; the BSDs report kibibytes.
    let scale = if cfg!(target_vendor = "apple") {
        1
    } else {
        1024
    };
    Ok(max_rss.saturating_mul(scale))
}

#[cfg(test)]
mod tests {
    use super::{KernelResidentMemoryProbe, parse_vm_rss_bytes};
    use quanta_index_core::ProcessMemoryProbePort as _;

    #[test]
    fn the_kernel_probe_reports_a_positive_resident_set_for_this_process() {
        let bytes = KernelResidentMemoryProbe
            .resident_bytes()
            .expect("the kernel reports this process");
        assert!(bytes > 0, "a running process has resident pages");
    }

    #[test]
    fn the_status_parser_reads_vm_rss_in_kibibytes_and_refuses_other_shapes() {
        let status = "Name:\tsearchd\nVmPeak:\t  12345 kB\nVmRSS:\t   6789 kB\nThreads:\t4\n";
        assert_eq!(parse_vm_rss_bytes(status).expect("parses"), 6789 * 1024);
        assert!(parse_vm_rss_bytes("Name:\tsearchd\n").is_err());
        assert!(parse_vm_rss_bytes("VmRSS:\t   many kB\n").is_err());
        assert!(parse_vm_rss_bytes("VmRSS:\t   12 pages\n").is_err());
    }
}
