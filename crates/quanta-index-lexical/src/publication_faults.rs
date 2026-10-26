//! Test-only cuts around physical pool, attempt journal, and root I/O.
//!
//! Controls are thread-local and compiled out of production. Error tests
//! inject an I/O refusal; crash tests pause at the acknowledged cut and let
//! the parent kill and reap the actual process.

#![expect(
    clippy::redundant_pub_crate,
    reason = "the private module exposes only crate-local test instrumentation"
)]

use std::cell::RefCell;
use std::io;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Cut {
    PoolObjectWrite,
    PoolObjectWriteCall,
    PoolObjectPartialWrite,
    PoolObjectFileSync,
    PoolObjectFileSyncCall,
    PoolObjectRename,
    PoolObjectDirectorySync,
    PoolGcDirectorySync,
    PoolOrphanCleanupDirectorySync,
    PoolUnlinkRecoveryDirectorySync,
    PoolFullBarrier,
    AttemptJournalWrite,
    AttemptJournalFileSync,
    AttemptJournalRename,
    AttemptJournalDirectorySync,
    HardScanCheckpointWrite,
    HardScanCheckpointFileSync,
    HardScanCheckpointRename,
    HardScanCheckpointDirectorySync,
    AttemptJournalAppendWrite,
    AttemptJournalAppendFileSync,
    InheritedObjectLink,
    DeltaCloneIntentCleanup,
    DeltaCloneDirectorySync,
    RootWrite,
    RootFileSync,
    RootRename,
    RootDirectorySync,
    RootTemporaryCleanup,
    RootTemporaryDirectorySync,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Side {
    Before,
    After,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct Case {
    cut: Cut,
    side: Side,
    occurrence: usize,
}

enum Mode {
    Error,
    OsError(i32),
    PartialWrite {
        prefix_bytes: usize,
        failure: PartialWriteFailure,
    },
    Pause(PathBuf),
    Wait(Arc<WaitPoint>),
}

#[derive(Clone, Copy)]
enum PartialWriteFailure {
    OsError(i32),
    WriteZero,
}

#[derive(Default)]
struct WaitPoint {
    reached: AtomicBool,
    released: AtomicBool,
}

struct Control {
    case: Case,
    mode: Mode,
    observed: usize,
    fired: bool,
}

thread_local! {
    static CONTROL: RefCell<Option<Control>> = const { RefCell::new(None) };
}

pub(crate) fn reach(cut: Cut, side: Side) -> io::Result<()> {
    CONTROL.with(|slot| {
        let mut borrowed = slot.borrow_mut();
        let Some(control) = borrowed.as_mut() else {
            return Ok(());
        };
        if control.case.cut != cut || control.case.side != side {
            return Ok(());
        }
        control.observed = control
            .observed
            .checked_add(1)
            .ok_or_else(|| io::Error::other("publication cut counter overflow"))?;
        if control.observed != control.case.occurrence {
            return Ok(());
        }
        control.fired = true;
        match &control.mode {
            Mode::Error => Err(io::Error::other(format!(
                "injected publication I/O refusal at {cut:?}/{side:?}"
            ))),
            Mode::OsError(code) => Err(io::Error::from_raw_os_error(*code)),
            Mode::PartialWrite { .. } => Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "partial-write fault requires the pool writer hook",
            )),
            Mode::Pause(marker) => {
                let pending = marker.with_extension("pending");
                std::fs::write(&pending, format!("{cut:?}/{side:?}"))?;
                std::fs::rename(&pending, marker)?;
                loop {
                    std::thread::park();
                }
            }
            Mode::Wait(point) => {
                point.reached.store(true, Ordering::SeqCst);
                while !point.released.load(Ordering::SeqCst) {
                    std::thread::park_timeout(Duration::from_millis(10));
                }
                Ok(())
            }
        }
    })
}

struct PrefixFaultWriter<'a, W> {
    inner: &'a mut W,
    remaining: usize,
    failure: PartialWriteFailure,
}

impl<W: io::Write> io::Write for PrefixFaultWriter<'_, W> {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        if self.remaining == 0 {
            return match self.failure {
                PartialWriteFailure::OsError(code) => Err(io::Error::from_raw_os_error(code)),
                PartialWriteFailure::WriteZero => Ok(0),
            };
        }
        let admitted = bytes
            .get(..bytes.len().min(self.remaining))
            .ok_or_else(|| io::Error::other("partial-write prefix exceeds input"))?;
        let written = self.inner.write(admitted)?;
        self.remaining = self
            .remaining
            .checked_sub(written)
            .ok_or_else(|| io::Error::other("partial-write writer exceeded prefix"))?;
        Ok(written)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.inner.flush()
    }
}

/// Writes a real prefix to the object temporary, then makes `write_all` see
/// a deterministic write failure. Production builds call `File::write_all`.
pub(crate) fn write_pool_object(file: &mut std::fs::File, bytes: &[u8]) -> io::Result<()> {
    reach(Cut::PoolObjectWriteCall, Side::Before)?;
    let fault = CONTROL.with(|slot| {
        let mut borrowed = slot.borrow_mut();
        let Some(control) = borrowed.as_mut() else {
            return Ok(None);
        };
        if control.case.cut != Cut::PoolObjectPartialWrite || control.case.side != Side::Before {
            return Ok(None);
        }
        control.observed = control
            .observed
            .checked_add(1)
            .ok_or_else(|| io::Error::other("publication cut counter overflow"))?;
        if control.observed != control.case.occurrence {
            return Ok(None);
        }
        control.fired = true;
        match &control.mode {
            Mode::PartialWrite {
                prefix_bytes,
                failure,
            } => Ok(Some((*prefix_bytes, *failure))),
            Mode::Error | Mode::OsError(_) | Mode::Pause(_) | Mode::Wait(_) => Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "pool partial-write cut requires a partial-write fault",
            )),
        }
    })?;
    let Some((prefix_bytes, failure)) = fault else {
        return file.write_all(bytes);
    };
    if prefix_bytes == 0 || prefix_bytes >= bytes.len() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "partial-write prefix must be nonempty and shorter than the object",
        ));
    }
    PrefixFaultWriter {
        inner: file,
        remaining: prefix_bytes,
        failure,
    }
    .write_all(bytes)
}

pub(crate) fn root_cut(cut: Cut, side: Side, path: &Path) -> io::Result<()> {
    if matches!(
        path.file_name().and_then(|name| name.to_str()),
        Some("root.cbor" | "manifest.cbor")
    ) && matches!(
        path.parent()
            .and_then(Path::file_name)
            .and_then(|name| name.to_str()),
        Some("file-authority" | "text-authority")
    ) {
        reach(cut, side)?;
    }
    Ok(())
}

pub(crate) fn inherited_cut(side: Side, path: &Path) -> io::Result<()> {
    if path
        .parent()
        .and_then(Path::file_name)
        .is_some_and(|name| name == "objects")
        && path
            .parent()
            .and_then(Path::parent)
            .and_then(Path::file_name)
            .is_some_and(|name| name == "file-authority")
    {
        reach(Cut::InheritedObjectLink, side)?;
    }
    Ok(())
}

pub(crate) fn reach_core(cut: Cut, side: Side) -> Result<(), quanta_index_core::CoreError> {
    reach(cut, side).map_err(|error| quanta_index_core::CoreError::Storage(error.to_string()))
}

pub(crate) fn root_cut_core(
    cut: Cut,
    side: Side,
    path: &Path,
) -> Result<(), quanta_index_core::CoreError> {
    root_cut(cut, side, path)
        .map_err(|error| quanta_index_core::CoreError::Storage(error.to_string()))
}

pub(crate) fn authority_cut_core(
    cut: Cut,
    side: Side,
    directory: &Path,
) -> Result<(), quanta_index_core::CoreError> {
    if directory
        .file_name()
        .is_some_and(|name| name == "file-authority" || name == "text-authority")
    {
        reach_core(cut, side)?;
    }
    Ok(())
}

pub(crate) fn inherited_cut_core(
    side: Side,
    path: &Path,
) -> Result<(), quanta_index_core::CoreError> {
    inherited_cut(side, path)
        .map_err(|error| quanta_index_core::CoreError::Storage(error.to_string()))
}

struct Guard {
    previous: Option<Control>,
    case: Case,
}

impl Guard {
    fn install(case: Case, mode: Mode) -> Self {
        let previous = CONTROL.with(|slot| {
            slot.replace(Some(Control {
                case,
                mode,
                observed: 0,
                fired: false,
            }))
        });
        Self { previous, case }
    }

    fn fired(&self) -> bool {
        CONTROL.with(|slot| {
            slot.borrow()
                .as_ref()
                .is_some_and(|control| control.case == self.case && control.fired)
        })
    }
}

impl Drop for Guard {
    fn drop(&mut self) {
        CONTROL.with(|slot| {
            drop(slot.replace(self.previous.take()));
        });
    }
}

pub(crate) fn with_error_at<T>(
    cut: Cut,
    side: Side,
    occurrence: usize,
    operation: impl FnOnce() -> T,
) -> (T, bool) {
    let guard = Guard::install(
        Case {
            cut,
            side,
            occurrence,
        },
        Mode::Error,
    );
    let outcome = operation();
    let fired = guard.fired();
    drop(guard);
    (outcome, fired)
}

pub(crate) fn with_pool_partial_write_at<T>(
    prefix_bytes: usize,
    occurrence: usize,
    operation: impl FnOnce() -> T,
) -> (T, bool) {
    let guard = Guard::install(
        Case {
            cut: Cut::PoolObjectPartialWrite,
            side: Side::Before,
            occurrence,
        },
        Mode::PartialWrite {
            prefix_bytes,
            failure: PartialWriteFailure::WriteZero,
        },
    );
    let outcome = operation();
    let fired = guard.fired();
    drop(guard);
    (outcome, fired)
}

mod tests;
