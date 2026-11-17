//! Own both observer workers through stop, partial startup and unwind.

use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use std::thread::{self, JoinHandle};

use anyhow::{Context as _, Result};

pub(super) struct ObservationWorkers {
    stopped: Arc<AtomicBool>,
    process: Option<JoinHandle<()>>,
    disk: Option<JoinHandle<()>>,
}

impl ObservationWorkers {
    pub(super) fn start(
        stopped: Arc<AtomicBool>,
        process: impl FnOnce(Arc<AtomicBool>) + Send + 'static,
        disk: impl FnOnce(Arc<AtomicBool>) + Send + 'static,
    ) -> Result<Self> {
        Self::start_with_disk_worker(stopped, process, |disk_stopped| {
            thread::Builder::new()
                .name("native-disk-observer".into())
                .spawn(move || disk(disk_stopped))
        })
    }

    fn start_with_disk_worker(
        stopped: Arc<AtomicBool>,
        process: impl FnOnce(Arc<AtomicBool>) + Send + 'static,
        spawn_disk: impl FnOnce(Arc<AtomicBool>) -> std::io::Result<JoinHandle<()>>,
    ) -> Result<Self> {
        let mut workers = Self {
            stopped,
            process: None,
            disk: None,
        };
        let process_stopped = Arc::clone(&workers.stopped);
        workers.process = Some(
            thread::Builder::new()
                .name("native-process-observer".into())
                .spawn(move || process(process_stopped))
                .context("spawn native process observer")?,
        );
        let disk_stopped = Arc::clone(&workers.stopped);
        workers.disk = Some(spawn_disk(disk_stopped).context("spawn native disk observer")?);
        // A second spawn failure drops workers and joins the first worker.
        Ok(workers)
    }

    pub(super) fn join(&mut self) -> Option<&'static str> {
        self.stopped.store(true, Ordering::Release);
        // Wake both before joining either. Neither keeps the other's sleep
        // alive, and a panic cannot skip the remaining join.
        for worker in [&self.process, &self.disk].into_iter().flatten() {
            worker.thread().unpark();
        }
        let mut failure = None;
        for (worker, message) in [
            (&mut self.process, "process_observer_thread_panicked"),
            (&mut self.disk, "disk_observer_thread_panicked"),
        ] {
            if let Some(worker) = worker.take()
                && worker.join().is_err()
            {
                let _first = failure.get_or_insert(message);
            }
        }
        failure
    }
}

impl Drop for ObservationWorkers {
    fn drop(&mut self) {
        let _failure = self.join();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use anyhow::ensure;

    #[test]
    fn second_worker_spawn_failure_cancels_and_joins_the_first_worker() -> Result<()> {
        use std::sync::mpsc;
        use std::time::Duration;

        let stopped = Arc::new(AtomicBool::new(false));
        let custody = Arc::new(());
        let worker_custody = Arc::clone(&custody);
        let (started, ready) = mpsc::sync_channel(1);
        let result = ObservationWorkers::start_with_disk_worker(
            Arc::clone(&stopped),
            move |stopped| {
                let _notified = started.send(());
                while !stopped.load(Ordering::Acquire) {
                    thread::park();
                }
                drop(worker_custody);
            },
            move |_stopped| {
                ready
                    .recv_timeout(Duration::from_secs(10))
                    .map_err(std::io::Error::other)?;
                Err(std::io::Error::other("controlled second spawn refusal"))
            },
        );
        let error = result.err().context("second worker unexpectedly started")?;
        let error = error
            .downcast_ref::<std::io::Error>()
            .context("spawn failure lost its original IO error")?;
        ensure!(error.to_string() == "controlled second spawn refusal");
        ensure!(stopped.load(Ordering::Acquire));
        ensure!(Arc::strong_count(&custody) == 1);
        Ok(())
    }

    fn panic_with_owned_custody(custody: Arc<()>) {
        std::panic::panic_any(custody);
    }

    #[test]
    fn worker_panic_still_joins_the_other_worker_and_drop_is_idempotent() -> Result<()> {
        let stopped = Arc::new(AtomicBool::new(false));
        let custody = Arc::new(());
        let process_custody = Arc::clone(&custody);
        let disk_custody = Arc::clone(&custody);
        let mut workers = ObservationWorkers::start(
            Arc::clone(&stopped),
            move |_stopped| {
                panic_with_owned_custody(process_custody);
            },
            move |stopped| {
                while !stopped.load(Ordering::Acquire) {
                    thread::park();
                }
                drop(disk_custody);
            },
        )?;
        ensure!(workers.join() == Some("process_observer_thread_panicked"));
        ensure!(stopped.load(Ordering::Acquire));
        drop(workers);
        ensure!(Arc::strong_count(&custody) == 1);
        Ok(())
    }
}
