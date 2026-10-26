//! Sample the harness and its observed process family without PID-only custody.
//!
//! This is a bounded, non-atomic census, not a continuous process-tree peak.

use std::sync::atomic::AtomicBool;
#[cfg(any(target_os = "linux", test))]
use std::sync::atomic::Ordering;
#[cfg(any(target_os = "linux", test))]
use std::{
    collections::{BTreeMap, BTreeSet, VecDeque},
    fs,
    path::Path,
};

#[cfg(any(target_os = "linux", test))]
use anyhow::Context as _;
use anyhow::Result;

pub(super) const MAX_PROCESSES: usize = 4096;

#[cfg(any(target_os = "linux", test))]
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
struct Identity {
    pid: u32,
    started: u64,
}

#[cfg(any(target_os = "linux", test))]
struct Process {
    identity: Identity,
    parent: u32,
}

pub(super) struct AggregateRss {
    #[cfg(any(target_os = "linux", test))]
    harness: u32,
    #[cfg(any(target_os = "linux", test))]
    daemon: u32,
    #[cfg(any(target_os = "linux", test))]
    roots: Option<[Identity; 2]>,
    #[cfg(any(target_os = "linux", test))]
    seen: BTreeSet<Identity>,
}

impl AggregateRss {
    pub(super) fn new(daemon: u32) -> Self {
        #[cfg(not(any(target_os = "linux", test)))]
        let _: u32 = daemon;
        Self {
            #[cfg(any(target_os = "linux", test))]
            harness: std::process::id(),
            #[cfg(any(target_os = "linux", test))]
            daemon,
            #[cfg(any(target_os = "linux", test))]
            roots: None,
            #[cfg(any(target_os = "linux", test))]
            seen: BTreeSet::new(),
        }
    }

    #[cfg(target_os = "linux")]
    pub(super) fn sample(&mut self, stopped: &AtomicBool) -> Result<u64> {
        self.sample_from(Path::new("/proc"), stopped)
    }

    #[cfg(not(target_os = "linux"))]
    pub(super) fn sample(&self, stopped: &AtomicBool) -> Result<u64> {
        let _: &AtomicBool = stopped;
        anyhow::bail!("aggregate process RSS unsupported on this platform")
    }

    #[cfg(any(target_os = "linux", test))]
    fn sample_from(&mut self, root: &Path, stopped: &AtomicBool) -> Result<u64> {
        let mut processes = BTreeMap::new();
        for entry in fs::read_dir(root).context("open process census")? {
            anyhow::ensure!(!stopped.load(Ordering::Acquire), "RSS census cancelled");
            let entry = entry?;
            let name = entry.file_name();
            let Some(name) = name.to_str() else {
                continue;
            };
            if name.is_empty() || !name.bytes().all(|byte| byte.is_ascii_digit()) {
                continue;
            }
            anyhow::ensure!(processes.len() < MAX_PROCESSES, "RSS process census bound");
            let pid = name.parse::<u32>()?;
            let process = read_process(root, pid)?;
            anyhow::ensure!(
                processes.insert(pid, process).is_none(),
                "duplicate process PID"
            );
        }
        let harness = processes
            .get(&self.harness)
            .context("harness absent from census")?;
        let daemon = processes
            .get(&self.daemon)
            .context("daemon absent from census")?;
        let roots = [harness.identity, daemon.identity];
        if let Some(original) = self.roots {
            anyhow::ensure!(roots == original, "RSS root process identity changed");
        } else {
            self.roots = Some(roots);
        }
        // Keep previously seen descendants even if they have been reparented.
        // A reused numeric PID with a different start time is never retained.
        let mut selected = BTreeSet::from(roots);
        for identity in &self.seen {
            if processes
                .get(&identity.pid)
                .is_some_and(|p| p.identity == *identity)
            {
                let _inserted = selected.insert(*identity);
            }
        }
        let mut children: BTreeMap<u32, Vec<Identity>> = BTreeMap::new();
        for process in processes.values() {
            children
                .entry(process.parent)
                .or_default()
                .push(process.identity);
        }
        let mut pending: VecDeque<_> = selected.iter().copied().collect();
        while let Some(identity) = pending.pop_front() {
            anyhow::ensure!(!stopped.load(Ordering::Acquire), "RSS census cancelled");
            if let Some(children) = children.get(&identity.pid) {
                for child in children {
                    if selected.insert(*child) {
                        pending.push_back(*child);
                    }
                }
            }
        }
        for identity in &selected {
            anyhow::ensure!(
                self.seen.contains(identity) || self.seen.len() < MAX_PROCESSES,
                "RSS observed process identity bound"
            );
            let _inserted = self.seen.insert(*identity);
        }
        let mut total = 0_u64;
        for identity in selected {
            anyhow::ensure!(!stopped.load(Ordering::Acquire), "RSS census cancelled");
            let status = fs::read_to_string(root.join(identity.pid.to_string()).join("status"))
                .context("read selected process RSS")?;
            let bytes = parse_rss(&status)?;
            anyhow::ensure!(
                read_process(root, identity.pid)?.identity == identity,
                "RSS process identity changed during sample"
            );
            total = total.checked_add(bytes).context("aggregate RSS overflow")?;
        }
        Ok(total)
    }
}

#[cfg(any(target_os = "linux", test))]
fn read_process(root: &Path, pid: u32) -> Result<Process> {
    let text = fs::read_to_string(root.join(pid.to_string()).join("stat"))
        .context("read process census identity")?;
    let (head, fields) = text.rsplit_once(')').context("process stat comm absent")?;
    let (claimed, _comm) = head.split_once(" (").context("process stat PID absent")?;
    anyhow::ensure!(claimed.parse::<u32>()? == pid, "process stat PID mismatch");
    let mut fields = fields.split_whitespace();
    let _state = fields.next().context("process state absent")?;
    let parent = fields.next().context("process parent absent")?.parse()?;
    let started = fields
        .nth(17)
        .context("process start time absent")?
        .parse()?;
    Ok(Process {
        identity: Identity { pid, started },
        parent,
    })
}

#[cfg(any(target_os = "linux", test))]
fn parse_rss(text: &str) -> Result<u64> {
    let mut values = text.lines().filter_map(|line| line.strip_prefix("VmRSS:"));
    let value = values.next().context("selected process VmRSS absent")?;
    anyhow::ensure!(values.next().is_none(), "duplicate process VmRSS");
    let mut fields = value.split_whitespace();
    let value = fields.next().context("process VmRSS value absent")?;
    anyhow::ensure!(
        value.bytes().all(|b| b.is_ascii_digit()),
        "invalid process VmRSS"
    );
    let kib = value.parse::<u64>()?;
    anyhow::ensure!(
        kib > 0 && fields.next() == Some("kB") && fields.next().is_none(),
        "invalid process VmRSS unit or unavailable RSS"
    );
    kib.checked_mul(1024).context("process RSS overflow")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn process(root: &Path, pid: u32, parent: u32, started: u64, kib: u64) -> Result<()> {
        let directory = root.join(pid.to_string());
        fs::create_dir_all(&directory)?;
        // Linux stat fields 3..22, including a comm containing spaces and ')'.
        let mut fields = vec!["S".to_owned(), parent.to_string()];
        fields.extend(std::iter::repeat_n("0".to_owned(), 17));
        fields.push(started.to_string());
        fs::write(
            directory.join("stat"),
            format!("{pid} (worker ) name) {}\n", fields.join(" ")),
        )?;
        fs::write(directory.join("status"), format!("VmRSS:\t{kib} kB\n"))?;
        Ok(())
    }

    fn observer() -> AggregateRss {
        AggregateRss {
            harness: 10,
            daemon: 20,
            roots: None,
            seen: BTreeSet::new(),
        }
    }

    #[test]
    fn family_sum_keeps_reparented_child_and_excludes_unrelated_reused_pid() -> Result<()> {
        let directory = tempfile::tempdir()?;
        let root = directory.path();
        process(root, 10, 1, 100, 1)?;
        process(root, 20, 10, 200, 2)?;
        process(root, 30, 20, 300, 4)?;
        process(root, 40, 1, 400, 128)?;
        let mut observed = observer();
        let stopped = AtomicBool::new(false);
        anyhow::ensure!(observed.sample_from(root, &stopped)? == 7 * 1024);
        process(root, 30, 1, 300, 8)?;
        anyhow::ensure!(observed.sample_from(root, &stopped)? == 11 * 1024);
        process(root, 30, 1, 301, 256)?;
        anyhow::ensure!(observed.sample_from(root, &stopped)? == 3 * 1024);
        process(root, 20, 10, 201, 2)?;
        anyhow::ensure!(observed.sample_from(root, &stopped).is_err());
        Ok(())
    }

    #[test]
    fn missing_rss_and_cancelled_census_are_errors_not_zero_samples() -> Result<()> {
        let directory = tempfile::tempdir()?;
        process(directory.path(), 10, 1, 100, 1)?;
        process(directory.path(), 20, 10, 200, 2)?;
        let mut observed = observer();
        anyhow::ensure!(
            observed
                .sample_from(directory.path(), &AtomicBool::new(true))
                .is_err()
        );
        fs::write(directory.path().join("20/status"), "Name: zombie\n")?;
        anyhow::ensure!(
            observed
                .sample_from(directory.path(), &AtomicBool::new(false))
                .is_err()
        );
        for invalid in [
            "VmRSS: 1 MB",
            "VmRSS: +1 kB",
            "VmRSS: 0 kB",
            "VmRSS: 1 kB\nVmRSS: 2 kB",
        ] {
            anyhow::ensure!(parse_rss(invalid).is_err());
        }
        Ok(())
    }
}
