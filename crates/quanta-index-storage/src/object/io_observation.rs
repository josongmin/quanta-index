//! Optional object-install IO and custody-port observation, never admission.
//!
//! Counts bytes returned by Write and the reached sync operations. It does not
//! measure device traffic, SQL/WAL operation counts, namespace opening or GC.
//! Custody time includes the reached authority/sink call, not SQL alone.
//! Snapshots require no operation in flight; invalid/poisoned accounting refuses.
//! Elapsed call sums exclude collector locking and may overlap across workers.

use quanta_index_core::index::IndexError;
use std::{
    io::{self, Write},
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

#[derive(Debug, Default)]
struct Origin;

impl PartialEq for Origin {
    fn eq(&self, other: &Self) -> bool {
        std::ptr::eq(self, other)
    }
}
impl Eq for Origin {}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ObjectIoSnapshot {
    pub write_calls: u64,
    pub written_bytes: u64,
    pub failed_writes: u64,
    /// Reached object writes that returned the kernel NOSPC errno.
    pub enospc_writes: u64,
    pub file_sync_calls: u64,
    pub failed_file_syncs: u64,
    pub directory_sync_calls: u64,
    pub failed_directory_syncs: u64,
    pub barrier_calls: u64,
    pub failed_barriers: u64,
    pub write_elapsed: Duration,
    pub file_sync_elapsed: Duration,
    pub directory_sync_elapsed: Duration,
    pub barrier_elapsed: Duration,
    pub reserve_calls: u64,
    /// Object entries passed to reached reserve calls, including refusals/replay.
    pub reserve_objects: u64,
    pub single_object_reserves: u64,
    pub failed_reserves: u64,
    pub confirm_calls: u64,
    /// Object entries passed to reached confirm calls, not newly installed files.
    pub confirm_objects: u64,
    pub single_object_confirms: u64,
    pub failed_confirms: u64,
    pub reserve_elapsed: Duration,
    pub confirm_elapsed: Duration,
    origin: Arc<Origin>,
}

impl ObjectIoSnapshot {
    fn validate_write_counts(&self) -> Result<(), IndexError> {
        if self.enospc_writes > self.failed_writes || self.failed_writes > self.write_calls {
            return Err(IndexError::BackendViolation(
                "object IO write observation is inconsistent".into(),
            ));
        }
        Ok(())
    }

    /// Both boundaries must name this same observer and a quiescent store.
    pub fn checked_since(self, prior: &Self) -> Result<Self, IndexError> {
        if !Arc::ptr_eq(&self.origin, &prior.origin) {
            return Err(IndexError::InvalidInput(
                "object IO observations have different owners".into(),
            ));
        }
        self.validate_write_counts()?;
        prior.validate_write_counts()?;
        let subtract = |now: u64, before: u64| {
            now.checked_sub(before).ok_or_else(|| {
                IndexError::BackendViolation("object IO observation regressed".into())
            })
        };
        let subtract_time = |now: Duration, before: Duration| {
            now.checked_sub(before).ok_or_else(|| {
                IndexError::BackendViolation("object IO elapsed observation regressed".into())
            })
        };
        let delta = Self {
            write_calls: subtract(self.write_calls, prior.write_calls)?,
            written_bytes: subtract(self.written_bytes, prior.written_bytes)?,
            failed_writes: subtract(self.failed_writes, prior.failed_writes)?,
            enospc_writes: subtract(self.enospc_writes, prior.enospc_writes)?,
            file_sync_calls: subtract(self.file_sync_calls, prior.file_sync_calls)?,
            failed_file_syncs: subtract(self.failed_file_syncs, prior.failed_file_syncs)?,
            directory_sync_calls: subtract(self.directory_sync_calls, prior.directory_sync_calls)?,
            failed_directory_syncs: subtract(
                self.failed_directory_syncs,
                prior.failed_directory_syncs,
            )?,
            barrier_calls: subtract(self.barrier_calls, prior.barrier_calls)?,
            failed_barriers: subtract(self.failed_barriers, prior.failed_barriers)?,
            write_elapsed: subtract_time(self.write_elapsed, prior.write_elapsed)?,
            file_sync_elapsed: subtract_time(self.file_sync_elapsed, prior.file_sync_elapsed)?,
            directory_sync_elapsed: subtract_time(
                self.directory_sync_elapsed,
                prior.directory_sync_elapsed,
            )?,
            barrier_elapsed: subtract_time(self.barrier_elapsed, prior.barrier_elapsed)?,
            reserve_calls: subtract(self.reserve_calls, prior.reserve_calls)?,
            reserve_objects: subtract(self.reserve_objects, prior.reserve_objects)?,
            single_object_reserves: subtract(
                self.single_object_reserves,
                prior.single_object_reserves,
            )?,
            failed_reserves: subtract(self.failed_reserves, prior.failed_reserves)?,
            confirm_calls: subtract(self.confirm_calls, prior.confirm_calls)?,
            confirm_objects: subtract(self.confirm_objects, prior.confirm_objects)?,
            single_object_confirms: subtract(
                self.single_object_confirms,
                prior.single_object_confirms,
            )?,
            failed_confirms: subtract(self.failed_confirms, prior.failed_confirms)?,
            reserve_elapsed: subtract_time(self.reserve_elapsed, prior.reserve_elapsed)?,
            confirm_elapsed: subtract_time(self.confirm_elapsed, prior.confirm_elapsed)?,
            origin: self.origin,
        };
        delta.validate_write_counts()?;
        Ok(delta)
    }

    /// Sum of reached call durations, not wall time under concurrent IO.
    pub fn elapsed_call_time(&self) -> Result<Duration, IndexError> {
        self.write_elapsed
            .checked_add(self.file_sync_elapsed)
            .and_then(|sum| sum.checked_add(self.directory_sync_elapsed))
            .and_then(|sum| sum.checked_add(self.barrier_elapsed))
            .ok_or_else(|| IndexError::BackendViolation("object IO elapsed sum overflowed".into()))
    }

    /// Reached IO plus custody-call sums, not wall time for nested/concurrent calls.
    pub fn elapsed_install_call_time(&self) -> Result<Duration, IndexError> {
        self.elapsed_call_time()?
            .checked_add(self.reserve_elapsed)
            .and_then(|sum| sum.checked_add(self.confirm_elapsed))
            .ok_or_else(|| {
                IndexError::BackendViolation("object install elapsed sum overflowed".into())
            })
    }
}

#[derive(Default)]
struct State {
    totals: ObjectIoSnapshot,
    in_flight: u64,
    invalid: bool,
}

#[derive(Default)]
pub struct ObjectIoObservation {
    state: Mutex<State>,
}

#[derive(Clone, Copy)]
pub(super) enum SyncKind {
    File,
    Directory,
    #[cfg(any(target_os = "macos", test))]
    Barrier,
}

#[derive(Clone, Copy)]
pub(super) enum CustodyKind {
    Reserve,
    Confirm,
}

#[derive(Clone, Copy)]
enum Operation {
    Write,
    Sync(SyncKind),
    Custody(CustodyKind, usize),
}

fn add(target: &mut u64, amount: u64) -> bool {
    target.checked_add(amount).is_some_and(|next| {
        *target = next;
        true
    })
}

fn add_elapsed(target: &mut Duration, elapsed: Option<Duration>) -> bool {
    elapsed
        .and_then(|elapsed| target.checked_add(elapsed))
        .is_some_and(|next| {
            *target = next;
            true
        })
}

impl ObjectIoObservation {
    pub fn snapshot(&self) -> Result<ObjectIoSnapshot, IndexError> {
        let state = self
            .state
            .lock()
            .map_err(|_poison| IndexError::Storage("object IO observer poisoned".into()))?;
        if state.invalid {
            return Err(IndexError::BackendViolation(
                "object IO observer overflowed or lost completion".into(),
            ));
        }
        if state.in_flight != 0 {
            return Err(IndexError::InvalidInput(
                "object IO snapshot requires quiescence".into(),
            ));
        }
        state.totals.validate_write_counts()?;
        Ok(state.totals.clone())
    }

    fn begin(&self, operation: Operation) -> Instant {
        if let Ok(mut state) = self.state.lock() {
            let counted = match operation {
                Operation::Write => add(&mut state.totals.write_calls, 1),
                Operation::Sync(SyncKind::File) => add(&mut state.totals.file_sync_calls, 1),
                Operation::Sync(SyncKind::Directory) => {
                    add(&mut state.totals.directory_sync_calls, 1)
                }
                #[cfg(any(target_os = "macos", test))]
                Operation::Sync(SyncKind::Barrier) => add(&mut state.totals.barrier_calls, 1),
                Operation::Custody(CustodyKind::Reserve, objects) => {
                    add(&mut state.totals.reserve_calls, 1)
                        && u64::try_from(objects)
                            .is_ok_and(|count| add(&mut state.totals.reserve_objects, count))
                        && (objects != 1 || add(&mut state.totals.single_object_reserves, 1))
                }
                Operation::Custody(CustodyKind::Confirm, objects) => {
                    add(&mut state.totals.confirm_calls, 1)
                        && u64::try_from(objects)
                            .is_ok_and(|count| add(&mut state.totals.confirm_objects, count))
                        && (objects != 1 || add(&mut state.totals.single_object_confirms, 1))
                }
            };
            let entered = add(&mut state.in_flight, 1);
            state.invalid |= !counted || !entered;
        }
        // The call clock starts after collector locking/accounting has ended.
        Instant::now()
    }

    fn end(
        &self,
        operation: Operation,
        written: Option<usize>,
        failed: bool,
        enospc_write: bool,
        elapsed: Option<Duration>,
    ) {
        if let Ok(mut state) = self.state.lock() {
            let counted = match operation {
                Operation::Write => {
                    let byte_count = written.is_none_or(|bytes| match u64::try_from(bytes) {
                        Ok(bytes) => add(&mut state.totals.written_bytes, bytes),
                        Err(_conversion_error) => false,
                    });
                    let errors = !failed || add(&mut state.totals.failed_writes, 1);
                    let enospc =
                        !enospc_write || (failed && add(&mut state.totals.enospc_writes, 1));
                    byte_count && errors && enospc
                }
                Operation::Sync(SyncKind::File) => {
                    !failed || add(&mut state.totals.failed_file_syncs, 1)
                }
                Operation::Sync(SyncKind::Directory) => {
                    !failed || add(&mut state.totals.failed_directory_syncs, 1)
                }
                #[cfg(any(target_os = "macos", test))]
                Operation::Sync(SyncKind::Barrier) => {
                    !failed || add(&mut state.totals.failed_barriers, 1)
                }
                Operation::Custody(CustodyKind::Reserve, _) => {
                    !failed || add(&mut state.totals.failed_reserves, 1)
                }
                Operation::Custody(CustodyKind::Confirm, _) => {
                    !failed || add(&mut state.totals.failed_confirms, 1)
                }
            };
            let timed = match operation {
                Operation::Write => add_elapsed(&mut state.totals.write_elapsed, elapsed),
                Operation::Sync(SyncKind::File) => {
                    add_elapsed(&mut state.totals.file_sync_elapsed, elapsed)
                }
                Operation::Sync(SyncKind::Directory) => {
                    add_elapsed(&mut state.totals.directory_sync_elapsed, elapsed)
                }
                #[cfg(any(target_os = "macos", test))]
                Operation::Sync(SyncKind::Barrier) => {
                    add_elapsed(&mut state.totals.barrier_elapsed, elapsed)
                }
                Operation::Custody(CustodyKind::Reserve, _) => {
                    add_elapsed(&mut state.totals.reserve_elapsed, elapsed)
                }
                Operation::Custody(CustodyKind::Confirm, _) => {
                    add_elapsed(&mut state.totals.confirm_elapsed, elapsed)
                }
            };
            if let Some(next) = state.in_flight.checked_sub(1) {
                state.in_flight = next;
            } else {
                state.invalid = true;
            }
            state.invalid |= !counted || !timed;
        }
    }

    pub(super) fn sync(
        &self,
        kind: SyncKind,
        operation: impl FnOnce() -> Result<(), IndexError>,
    ) -> Result<(), IndexError> {
        self.call(Operation::Sync(kind), operation)
    }

    pub(super) fn custody(
        &self,
        kind: CustodyKind,
        objects: usize,
        operation: impl FnOnce() -> Result<(), IndexError>,
    ) -> Result<(), IndexError> {
        self.call(Operation::Custody(kind, objects), operation)
    }

    fn call(
        &self,
        kind: Operation,
        operation: impl FnOnce() -> Result<(), IndexError>,
    ) -> Result<(), IndexError> {
        let started = self.begin(kind);
        let result = operation();
        let elapsed = Instant::now().checked_duration_since(started);
        self.end(kind, None, result.is_err(), false, elapsed);
        result
    }

    pub(super) fn write_all(&self, writer: &mut impl Write, bytes: &[u8]) -> io::Result<()> {
        struct ObservedWriter<'a, W> {
            inner: &'a mut W,
            observer: &'a ObjectIoObservation,
        }
        impl<W: Write> Write for ObservedWriter<'_, W> {
            fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
                let started = self.observer.begin(Operation::Write);
                let result = self.inner.write(bytes);
                let elapsed = Instant::now().checked_duration_since(started);
                let written = match &result {
                    Ok(bytes) => Some(*bytes),
                    Err(_io_error) => None,
                };
                let enospc_write = matches!(
                    &result,
                    Err(error)
                        if error.raw_os_error()
                            == Some(rustix::io::Errno::NOSPC.raw_os_error())
                );
                self.observer.end(
                    Operation::Write,
                    written,
                    result.is_err(),
                    enospc_write,
                    elapsed,
                );
                result
            }
            fn flush(&mut self) -> io::Result<()> {
                self.inner.flush()
            }
        }
        ObservedWriter {
            inner: writer,
            observer: self,
        }
        .write_all(bytes)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    macro_rules! check {
        ($condition:expr) => {
            if !$condition {
                return Err(IndexError::BackendViolation(stringify!($condition).into()));
            }
        };
    }

    struct PartialThenRefuse {
        calls: u64,
    }
    impl Write for PartialThenRefuse {
        fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
            self.calls = self.calls.saturating_add(1);
            if self.calls == 1 {
                Ok(bytes.len().min(2))
            } else {
                Err(io::Error::new(io::ErrorKind::BrokenPipe, "fixed refusal"))
            }
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    #[test]
    fn partial_write_and_original_error_have_exact_prefix_counts() -> Result<(), IndexError> {
        let observer = ObjectIoObservation::default();
        let prior = observer.snapshot()?;
        let mut writer = PartialThenRefuse { calls: 0 };
        let result = observer.write_all(&mut writer, b"abcde");
        check!(
            matches!(result, Err(error) if error.kind() == io::ErrorKind::BrokenPipe && error.to_string() == "fixed refusal")
        );
        let sample = observer.snapshot()?.checked_since(&prior)?;
        check!(
            sample.write_calls == 2
                && sample.written_bytes == 2
                && sample.failed_writes == 1
                && sample.enospc_writes == 0
        );
        check!(
            sample.file_sync_calls == 0
                && sample.directory_sync_calls == 0
                && sample.barrier_calls == 0
        );
        Ok(())
    }

    struct PartialThenEnospc {
        calls: u8,
    }

    impl Write for PartialThenEnospc {
        fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
            self.calls = self
                .calls
                .checked_add(1)
                .ok_or_else(|| io::Error::other("fixture write count overflow"))?;
            if self.calls == 1 {
                Ok(bytes.len().min(2))
            } else {
                Err(io::Error::from_raw_os_error(
                    rustix::io::Errno::NOSPC.raw_os_error(),
                ))
            }
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    #[test]
    fn raw_nospc_after_partial_write_preserves_errno_and_bounded_delta() -> Result<(), IndexError> {
        let observer = ObjectIoObservation::default();
        let before = observer.snapshot()?;
        let mut writer = PartialThenEnospc { calls: 0 };
        let result = observer.write_all(&mut writer, b"abcde");
        check!(matches!(
            result,
            Err(error)
                if error.raw_os_error() == Some(rustix::io::Errno::NOSPC.raw_os_error())
        ));
        let after = observer.snapshot()?;
        let sample = after.clone().checked_since(&before)?;
        check!(
            writer.calls == 2
                && sample.write_calls == 2
                && sample.written_bytes == 2
                && sample.failed_writes == 1
                && sample.enospc_writes == 1
        );
        check!(observer.snapshot()?.checked_since(&after)?.enospc_writes == 0);
        let foreign = ObjectIoObservation::default();
        check!(matches!(
            after.clone().checked_since(&foreign.snapshot()?),
            Err(IndexError::InvalidInput(_))
        ));
        // Both endpoints can look valid while their forged difference cannot.
        let mut forged_prior = before;
        forged_prior.write_calls = 1;
        forged_prior.failed_writes = 1;
        check!(matches!(
            after.clone().checked_since(&forged_prior),
            Err(IndexError::BackendViolation(_))
        ));
        let mut future = after;
        future.enospc_writes = future
            .enospc_writes
            .checked_add(1)
            .ok_or_else(|| IndexError::BackendViolation("fixture count overflow".into()))?;
        check!(matches!(
            observer.snapshot()?.checked_since(&future),
            Err(IndexError::BackendViolation(_))
        ));
        Ok(())
    }

    struct FailOnceWriter {
        failure: Option<io::Error>,
        output: Vec<u8>,
    }

    impl Write for FailOnceWriter {
        fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
            if let Some(error) = self.failure.take() {
                return Err(error);
            }
            self.output.write(bytes)
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    struct ZeroWriter;

    impl Write for ZeroWriter {
        fn write(&mut self, _bytes: &[u8]) -> io::Result<usize> {
            Ok(0)
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    #[test]
    fn interrupted_other_error_and_write_zero_are_not_nospc() -> Result<(), IndexError> {
        let observer = ObjectIoObservation::default();
        let before = observer.snapshot()?;
        let mut interrupted = FailOnceWriter {
            failure: Some(io::Error::from(io::ErrorKind::Interrupted)),
            output: Vec::new(),
        };
        observer
            .write_all(&mut interrupted, b"ok")
            .map_err(|error| IndexError::Storage(error.to_string()))?;
        check!(interrupted.output == b"ok");
        let mut lookalike = FailOnceWriter {
            failure: Some(io::Error::other("No space left on device")),
            output: Vec::new(),
        };
        check!(matches!(
            observer.write_all(&mut lookalike, b"x"),
            Err(error) if error.raw_os_error().is_none()
        ));
        check!(matches!(
            observer.write_all(&mut ZeroWriter, b"x"),
            Err(error) if error.kind() == io::ErrorKind::WriteZero
        ));
        let sample = observer.snapshot()?.checked_since(&before)?;
        check!(
            sample.write_calls == 4
                && sample.written_bytes == 2
                && sample.failed_writes == 2
                && sample.enospc_writes == 0
        );
        Ok(())
    }

    #[test]
    fn enospc_counter_overflow_refuses_snapshot_without_changing_write_error()
    -> Result<(), IndexError> {
        let observer = ObjectIoObservation::default();
        observer
            .state
            .lock()
            .map_err(|_poison| IndexError::Storage("fixture lock poisoned".into()))?
            .totals
            .enospc_writes = u64::MAX;
        let mut writer = PartialThenEnospc { calls: 1 };
        check!(matches!(
            observer.write_all(&mut writer, b"x"),
            Err(error)
                if error.raw_os_error() == Some(rustix::io::Errno::NOSPC.raw_os_error())
        ));
        check!(matches!(
            observer.snapshot(),
            Err(IndexError::BackendViolation(_))
        ));
        Ok(())
    }

    #[test]
    fn reached_syncs_preserve_failure_identity_and_snapshots_refuse_foreign_or_busy()
    -> Result<(), IndexError> {
        let observer = ObjectIoObservation::default();
        let prior = observer.snapshot()?;
        observer.sync(SyncKind::File, || Ok(()))?;
        check!(matches!(
            observer.sync(SyncKind::Directory, || Err(IndexError::FenceLost)),
            Err(IndexError::FenceLost)
        ));
        observer.sync(SyncKind::Barrier, || Ok(()))?;
        let sample = observer.snapshot()?.checked_since(&prior)?;
        check!(sample.file_sync_calls == 1 && sample.failed_file_syncs == 0);
        check!(sample.directory_sync_calls == 1 && sample.failed_directory_syncs == 1);
        check!(sample.barrier_calls == 1 && sample.failed_barriers == 0);
        let foreign = ObjectIoObservation::default();
        check!(matches!(
            observer.snapshot()?.checked_since(&foreign.snapshot()?),
            Err(IndexError::InvalidInput(_))
        ));
        let _started = observer.begin(Operation::Write);
        check!(matches!(
            observer.snapshot(),
            Err(IndexError::InvalidInput(_))
        ));
        observer.end(
            Operation::Write,
            Some(0),
            false,
            false,
            Some(Duration::ZERO),
        );
        let _sample = observer.snapshot()?;
        Ok(())
    }

    #[test]
    fn overflow_cannot_publish_zero_or_change_the_io_result() -> Result<(), IndexError> {
        let observer = ObjectIoObservation::default();
        observer
            .state
            .lock()
            .map_err(|_poison| IndexError::Storage("fixture lock poisoned".into()))?
            .totals
            .written_bytes = u64::MAX;
        let mut output = Vec::new();
        observer
            .write_all(&mut output, b"a")
            .map_err(|error| IndexError::Storage(error.to_string()))?;
        check!(output.as_slice() == b"a");
        check!(matches!(
            observer.snapshot(),
            Err(IndexError::BackendViolation(_))
        ));
        for kind in [CustodyKind::Reserve, CustodyKind::Confirm] {
            let custody = ObjectIoObservation::default();
            {
                let mut state = custody
                    .state
                    .lock()
                    .map_err(|_poison| IndexError::Storage("fixture lock poisoned".into()))?;
                match kind {
                    CustodyKind::Reserve => state.totals.reserve_objects = u64::MAX,
                    CustodyKind::Confirm => state.totals.confirm_objects = u64::MAX,
                }
            }
            check!(matches!(
                custody.custody(kind, 2, || Err(IndexError::FenceLost)),
                Err(IndexError::FenceLost)
            ));
            check!(matches!(
                custody.snapshot(),
                Err(IndexError::BackendViolation(_))
            ));
        }
        Ok(())
    }

    struct DelayedWriter {
        output: Vec<u8>,
        delay: Duration,
    }

    impl Write for DelayedWriter {
        fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
            std::thread::sleep(self.delay);
            self.output.write(bytes)
        }

        fn flush(&mut self) -> io::Result<()> {
            self.output.flush()
        }
    }

    #[test]
    fn elapsed_calls_include_failed_sync_and_remain_within_serial_wall_time()
    -> Result<(), IndexError> {
        let observer = ObjectIoObservation::default();
        let before = observer.snapshot()?;
        let started = Instant::now();
        let delay = Duration::from_millis(2);
        let mut writer = DelayedWriter {
            output: Vec::new(),
            delay,
        };
        observer
            .write_all(&mut writer, b"gold")
            .map_err(|error| IndexError::Storage(error.to_string()))?;
        check!(writer.output.as_slice() == b"gold");
        observer.sync(SyncKind::File, || {
            std::thread::sleep(delay);
            Ok(())
        })?;
        check!(matches!(
            observer.sync(SyncKind::Directory, || {
                std::thread::sleep(delay);
                Err(IndexError::FenceLost)
            }),
            Err(IndexError::FenceLost)
        ));
        observer.sync(SyncKind::Barrier, || {
            std::thread::sleep(delay);
            Ok(())
        })?;
        observer.custody(CustodyKind::Reserve, 2, || {
            std::thread::sleep(delay);
            Ok(())
        })?;
        check!(matches!(
            observer.custody(CustodyKind::Confirm, 2, || {
                std::thread::sleep(delay);
                Err(IndexError::FenceLost)
            }),
            Err(IndexError::FenceLost)
        ));
        let wall = Instant::now()
            .checked_duration_since(started)
            .ok_or_else(|| IndexError::BackendViolation("fixture clock regressed".into()))?;
        let sample = observer.snapshot()?.checked_since(&before)?;
        check!(sample.write_calls == 1 && sample.written_bytes == 4);
        check!(sample.file_sync_calls == 1 && sample.directory_sync_calls == 1);
        check!(sample.barrier_calls == 1 && sample.failed_directory_syncs == 1);
        check!(sample.write_elapsed >= delay && sample.file_sync_elapsed >= delay);
        check!(sample.directory_sync_elapsed >= delay && sample.barrier_elapsed >= delay);
        check!(sample.reserve_calls == 1 && sample.confirm_calls == 1);
        check!(sample.reserve_objects == 2 && sample.confirm_objects == 2);
        check!(sample.single_object_reserves == 0 && sample.single_object_confirms == 0);
        check!(sample.failed_reserves == 0 && sample.failed_confirms == 1);
        check!(sample.reserve_elapsed >= delay && sample.confirm_elapsed >= delay);
        check!(sample.elapsed_call_time()? <= wall);
        check!(sample.elapsed_install_call_time()? <= wall);
        let mut future = observer.snapshot()?;
        future.write_elapsed = Duration::MAX;
        check!(matches!(
            observer.snapshot()?.checked_since(&future),
            Err(IndexError::BackendViolation(_))
        ));
        Ok(())
    }

    #[test]
    fn elapsed_overflow_or_missing_clock_refuses_observation() -> Result<(), IndexError> {
        let observer = ObjectIoObservation::default();
        observer
            .state
            .lock()
            .map_err(|_poison| IndexError::Storage("fixture lock poisoned".into()))?
            .totals
            .file_sync_elapsed = Duration::MAX;
        // Timing overflow invalidates observation, not successful product IO.
        observer.sync(SyncKind::File, || {
            std::thread::sleep(Duration::from_millis(1));
            Ok(())
        })?;
        check!(matches!(
            observer.snapshot(),
            Err(IndexError::BackendViolation(_))
        ));
        let missing_clock = ObjectIoObservation::default();
        let _started = missing_clock.begin(Operation::Write);
        missing_clock.end(Operation::Write, Some(0), false, false, None);
        check!(matches!(
            missing_clock.snapshot(),
            Err(IndexError::BackendViolation(_))
        ));
        Ok(())
    }
}
