//! Optional callback-local refusal observations; never a terminal SQL cause.
//!
//! Only dispatched callbacks on registered adapters are covered. Registration,
//! router lookup/closing and native global-handle limits are not covered. Direct
//! `Refusal` paths have no attributable `CoreError` detail.

use std::cell::RefCell;
use std::ffi::OsStr;
use std::num::NonZeroU64;
use std::sync::atomic::{AtomicUsize, Ordering};

use quanta_index_contract::SearchPlaneErrorCodeV2;
use quanta_index_core::CoreError;
use rusqlite::ffi::quanta_capacity_vfs::{EventKind, Refusal};

const ENV: &str = "QUANTA_INDEX_NATIVE_WORK_DIAGNOSTICS_V1";
const SAMPLE_LIMIT: usize = 8;
static SAMPLED: AtomicUsize = AtomicUsize::new(0);

#[cfg(test)]
struct FailedWriter;
#[cfg(test)]
impl std::io::Write for FailedWriter {
    fn write(&mut self, _bytes: &[u8]) -> std::io::Result<usize> {
        Err(std::io::Error::other("closed"))
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

struct Callback {
    enabled: bool,
    scope: NonZeroU64,
    handle: u64,
    kind: EventKind,
    operation: i32,
    capacity_core: Option<CoreError>,
    io_core: Option<CoreError>,
}

thread_local! {
    static CURRENT: RefCell<Option<Callback>> = const { RefCell::new(None) };
    #[cfg(test)]
    static TEST_LINES: RefCell<Option<Vec<Vec<u8>>>> = const { RefCell::new(None) };
    #[cfg(test)]
    static TEST_WRITE_FAILURE: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

// Reentrancy or TLS teardown can omit this optional observation. It never
// substitutes a result or supplies admission authority to the callback.
fn update_current<T>(update: impl FnOnce(&mut Option<Callback>) -> T) -> Option<T> {
    CURRENT
        .try_with(|slot| {
            slot.try_borrow_mut()
                .map_or_else(|_reentered| None, |mut slot| Some(update(&mut slot)))
        })
        .map_or_else(|_tls_destroyed| None, std::convert::identity)
}

pub(super) fn enabled_at_registration() -> bool {
    std::env::var_os(ENV).as_deref() == Some(OsStr::new("1"))
}

pub(super) struct Frame {
    previous: Option<Callback>,
    installed: bool,
}

impl Frame {
    pub(super) fn enter(
        enabled: bool,
        scope: NonZeroU64,
        handle: u64,
        kind: EventKind,
        operation: i32,
    ) -> Self {
        // A disabled nested adapter must mask the enabled outer callback.
        // The ordinary disabled path only reads TLS and allocates nothing.
        if !enabled
            && !CURRENT
                .try_with(|slot| {
                    slot.try_borrow()
                        .map_or_else(|_reentered| false, |slot| slot.is_some())
                })
                .map_or_else(|_tls_destroyed| false, std::convert::identity)
        {
            return Self {
                previous: None,
                installed: false,
            };
        }
        let new = Callback {
            enabled,
            scope,
            handle,
            kind,
            operation,
            capacity_core: None,
            io_core: None,
        };
        update_current(|slot| slot.replace(new)).map_or_else(
            || Self {
                previous: None,
                installed: false,
            },
            |previous| Self {
                previous,
                installed: true,
            },
        )
    }

    fn take(&mut self) -> Option<Callback> {
        if !self.installed {
            return None;
        }
        let taken = update_current(|slot| {
            let captured = slot.take();
            *slot = self.previous.take();
            captured
        });
        if taken.is_some() {
            self.installed = false;
        }
        taken.flatten()
    }

    pub(super) fn finish<T>(mut self, result: &Result<T, Refusal>) {
        if let Some(captured) = self.take().filter(|captured| captured.enabled) {
            let returned = result.as_ref().err().copied();
            let _observer = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                emit(captured, returned);
            }));
        }
    }
}

impl Drop for Frame {
    fn drop(&mut self) {
        // Panic unwinding may omit diagnostics; it must still restore nesting.
        let _discarded = self.take();
    }
}

pub(super) fn observe(error: CoreError, mapped: Refusal) {
    let _observed = CURRENT.try_with(|slot| {
        if let Ok(mut slot) = slot.try_borrow_mut()
            && let Some(current) = slot.as_mut()
            && current.enabled
        {
            match mapped {
                Refusal::Capacity
                    if matches!(
                        &error,
                        CoreError::Typed {
                            code: SearchPlaneErrorCodeV2::IngestResourceBudgetExceeded,
                            ..
                        }
                    ) =>
                {
                    if current.capacity_core.is_none() {
                        current.capacity_core = Some(error);
                    }
                }
                Refusal::Io => {
                    if current.io_core.is_none() {
                        current.io_core = Some(error);
                    }
                }
                Refusal::Capacity
                | Refusal::OutOfMemory
                | Refusal::CannotOpen
                | Refusal::Busy
                | Refusal::UnsupportedFileControl => {}
            }
        }
    });
}

fn bounded_detail(message: &str) -> (&str, bool) {
    if message.len() <= 256 {
        return (message, false);
    }
    let end = message
        .char_indices()
        .map(|(offset, _)| offset)
        .take_while(|offset| *offset <= 256)
        .last()
        .unwrap_or(0);
    (message.get(..end).unwrap_or(""), true)
}

fn sample() -> Option<usize> {
    #[cfg(test)]
    if let Some(slot) = TEST_LINES
        .try_with(|lines| {
            lines.try_borrow().map_or_else(
                |_reentered| None,
                |lines| {
                    lines.as_ref().map(|lines| {
                        lines
                            .len()
                            .checked_add(1)
                            .filter(|next| *next <= SAMPLE_LIMIT)
                    })
                },
            )
        })
        .map_or_else(|_tls_destroyed| None, std::convert::identity)
    {
        return slot;
    }
    claim_sample(&SAMPLED)
}

fn claim_sample(counter: &AtomicUsize) -> Option<usize> {
    counter
        .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |old| {
            old.checked_add(1).filter(|next| *next <= SAMPLE_LIMIT)
        })
        .map_or_else(|_sample_limit_reached| None, |old| old.checked_add(1))
}

fn emit(callback: Callback, returned: Option<Refusal>) {
    // A callback-local observation is not a terminal SQL cause. A CoreError
    // only describes this refusal when its mapping matches the returned code.
    if !matches!(returned, Some(Refusal::Capacity | Refusal::Io)) {
        return;
    }
    let Some(ordinal) = sample() else { return };
    let value = if returned == Some(Refusal::Capacity) {
        // Preserve the original Capacity schema and typed-budget semantics.
        let (type_code, detail, truncated, mapped, origin) = match callback.capacity_core {
            Some(CoreError::Typed { code, message }) => {
                let (detail, truncated) = bounded_detail(&message);
                (
                    Some(code.to_string()),
                    Some(detail.to_owned()),
                    truncated,
                    Some(Refusal::Capacity),
                    "core_error_observed",
                )
            }
            _ => (None, None, false, returned, "capacity_without_core_error"),
        };
        serde_json::json!({
            "event": "native_sqlite_vfs_refusal_observed",
            "origin": origin,
            "scope_id": callback.scope.get(),
            "handle_id": callback.handle,
            "callback_kind": format!("{:?}", callback.kind),
            "core_variant": type_code.as_ref().map(|_| "Typed"),
            "type_code": type_code,
            "detail": detail,
            "detail_truncated": truncated,
            "mapped_refusal": mapped.map(|value| format!("{value:?}")),
            "callback_returned": returned.map(|value| format!("{value:?}")),
            "sample_ordinal": ordinal,
            "sample_limit": SAMPLE_LIMIT,
        })
    } else {
        let (variant, type_code, detail, truncated, raw_errno, origin) = callback.io_core.map_or(
            (None, None, None, false, None, "io_without_core_error"),
            |error| {
                let (variant, type_code, message, raw_errno) = match &error {
                    CoreError::InvalidContract(message) => ("InvalidContract", None, message, None),
                    CoreError::Typed { code, message } => {
                        ("Typed", Some(code.to_string()), message, None)
                    }
                    CoreError::NotReady(message) => ("NotReady", None, message, None),
                    CoreError::NotImplemented(message) => ("NotImplemented", None, message, None),
                    CoreError::NotFound(message) => ("NotFound", None, message, None),
                    CoreError::Storage(message) => ("Storage", None, message, None),
                    CoreError::StorageIo {
                        message,
                        raw_os_error,
                        ..
                    } => ("StorageIo", None, message, *raw_os_error),
                };
                let (detail, truncated) = bounded_detail(message);
                (
                    Some(variant),
                    type_code,
                    Some(detail.to_owned()),
                    truncated,
                    raw_errno,
                    "core_error_observed",
                )
            },
        );
        serde_json::json!({
            "event": "native_sqlite_vfs_refusal_observed",
            "origin": origin,
            "scope_id": callback.scope.get(),
            "handle_id": callback.handle,
            "callback_kind": format!("{:?}", callback.kind),
            "operation": callback.operation,
            "core_variant": variant,
            "type_code": type_code,
            "detail": detail,
            "detail_truncated": truncated,
            "raw_errno": raw_errno,
            "mapped_refusal": if variant.is_some() { Some("Io") } else { None },
            "callback_returned": "Io",
            "sample_ordinal": ordinal,
            "sample_limit": SAMPLE_LIMIT,
        })
    };
    let Ok(mut line) = serde_json::to_vec(&value) else {
        return;
    };
    line.push(b'\n');
    #[cfg(test)]
    if TEST_WRITE_FAILURE
        .try_with(std::cell::Cell::get)
        .map_or_else(|_tls_destroyed| false, std::convert::identity)
    {
        ignore_write_failure(&mut FailedWriter, &line);
        return;
    }
    #[cfg(test)]
    if TEST_LINES.with(|slot| {
        if let Ok(mut slot) = slot.try_borrow_mut()
            && let Some(lines) = slot.as_mut()
        {
            lines.push(line.clone());
            return true;
        }
        false
    }) {
        return;
    }
    ignore_write_failure(&mut std::io::stderr().lock(), &line);
}

fn ignore_write_failure(output: &mut impl std::io::Write, line: &[u8]) {
    let _written = output.write_all(line);
}

#[cfg(test)]
pub(super) fn capture_test_lines<T>(run: impl FnOnce() -> T) -> (T, Vec<Vec<u8>>) {
    struct Restore(Option<Vec<Vec<u8>>>);
    impl Drop for Restore {
        fn drop(&mut self) {
            TEST_LINES.with(|slot| *slot.borrow_mut() = self.0.take());
        }
    }
    let previous = TEST_LINES.with(|slot| slot.replace(Some(Vec::new())));
    let restore = Restore(previous);
    let result = run();
    let lines: Vec<Vec<u8>> = TEST_LINES.with(|slot| slot.borrow_mut().take().unwrap_or_default());
    drop(restore);
    (result, lines)
}

#[cfg(test)]
mod tests {
    use super::*;

    type TestResult = Result<(), Box<dyn std::error::Error>>;

    fn require(condition: bool, message: &str) -> TestResult {
        if condition {
            Ok(())
        } else {
            Err(std::io::Error::other(message).into())
        }
    }

    #[test]
    fn concurrent_sample_claims_share_one_eight_event_bound() -> TestResult {
        let counter = std::sync::Arc::new(AtomicUsize::new(0));
        let start = std::sync::Arc::new(std::sync::Barrier::new(32));
        let children: Vec<_> = (0..32)
            .map(|_| {
                let counter = std::sync::Arc::clone(&counter);
                let start = std::sync::Arc::clone(&start);
                std::thread::spawn(move || {
                    let _arrival = start.wait();
                    claim_sample(&counter)
                })
            })
            .collect();
        let mut ordinals = Vec::new();
        for child in children {
            if let Some(ordinal) = child.join().map_err(|payload| {
                std::io::Error::other(format!("sample claimant panicked: {payload:?}"))
            })? {
                ordinals.push(ordinal);
            }
        }
        ordinals.sort_unstable();
        require(
            ordinals == (1..=8).collect::<Vec<_>>(),
            "sample ordinals differ",
        )?;
        require(counter.load(Ordering::Relaxed) == 8, "sample count differs")?;
        require(claim_sample(&counter).is_none(), "sample bound exceeded")?;
        Ok(())
    }

    #[test]
    fn callback_frames_keep_exact_observations_isolated_and_bounded() -> TestResult {
        let scope = NonZeroU64::new(17).ok_or("nonzero scope")?;
        let ((), lines) = capture_test_lines(|| {
            let off = Frame::enter(false, scope, 1, EventKind::BeforeWrite, 0);
            observe(CoreError::Storage("off".into()), Refusal::Io);
            off.finish(&Err::<(), _>(Refusal::Io));
            let outer = Frame::enter(true, scope, 2, EventKind::BeforeWrite, 0);
            observe(
                CoreError::Typed {
                    code: SearchPlaneErrorCodeV2::IngestResourceBudgetExceeded,
                    message: "é".repeat(130),
                },
                Refusal::Capacity,
            );
            let masked = Frame::enter(false, scope, 20, EventKind::BeforeWrite, 0);
            observe(
                CoreError::Typed {
                    code: SearchPlaneErrorCodeV2::IngestResourceBudgetExceeded,
                    message: "masked".into(),
                },
                Refusal::Capacity,
            );
            masked.finish(&Err::<(), _>(Refusal::Capacity));
            let inner = Frame::enter(true, scope, 3, EventKind::AfterSync, 0);
            observe(
                CoreError::Typed {
                    code: SearchPlaneErrorCodeV2::IngestResourceBudgetExceeded,
                    message: "inner".into(),
                },
                Refusal::Capacity,
            );
            inner.finish(&Err::<(), _>(Refusal::Capacity));
            outer.finish(&Err::<(), _>(Refusal::Capacity));
            Frame::enter(true, scope, 4, EventKind::BeforeOpen, 0)
                .finish(&Err::<(), _>(Refusal::Capacity));
            Frame::enter(true, scope, 5, EventKind::BeforeOpen, 0).finish(&Ok::<(), Refusal>(()));
        });
        require(lines.len() == 3, "unexpected callback observation count")?;
        let rows: Vec<serde_json::Value> = lines
            .iter()
            .map(|line| serde_json::from_slice(line))
            .collect::<Result<_, _>>()?;
        let inner = rows.first().ok_or("missing inner row")?;
        let outer = rows.get(1).ok_or("missing outer row")?;
        let direct = rows.get(2).ok_or("missing direct row")?;
        require(
            inner.get("handle_id").and_then(serde_json::Value::as_u64) == Some(3),
            "inner handle differs",
        )?;
        require(
            outer.get("handle_id").and_then(serde_json::Value::as_u64) == Some(2),
            "outer handle differs",
        )?;
        require(
            outer.get("scope_id").and_then(serde_json::Value::as_u64) == Some(17),
            "scope differs",
        )?;
        require(
            outer
                .get("mapped_refusal")
                .and_then(serde_json::Value::as_str)
                == Some("Capacity"),
            "mapped refusal differs",
        )?;
        require(
            outer
                .get("detail")
                .and_then(serde_json::Value::as_str)
                .is_some_and(|detail| detail.len() <= 256),
            "detail cap exceeded",
        )?;
        require(
            outer
                .get("detail_truncated")
                .and_then(serde_json::Value::as_bool)
                == Some(true),
            "truncation flag missing",
        )?;
        require(
            !outer
                .as_object()
                .is_some_and(|row| row.contains_key("operation") || row.contains_key("raw_errno")),
            "Capacity event schema changed",
        )?;
        require(
            direct.get("origin").and_then(serde_json::Value::as_str)
                == Some("capacity_without_core_error"),
            "direct refusal origin differs",
        )?;
        require(
            direct.get("detail").is_some_and(serde_json::Value::is_null),
            "direct refusal acquired detail",
        )?;
        Ok(())
    }

    #[test]
    fn io_callback_reports_only_its_matching_observed_cause() -> TestResult {
        let scope = NonZeroU64::new(23).ok_or("nonzero scope")?;
        let ((), lines) = capture_test_lines(|| {
            let off = Frame::enter(false, scope, 1, EventKind::FdRetired, 0);
            observe(CoreError::Storage("disabled".into()), Refusal::Io);
            off.finish(&Err::<(), _>(Refusal::Io));

            let outer = Frame::enter(true, scope, 2, EventKind::FdRetired, 0);
            observe(CoreError::Storage("outer".into()), Refusal::Io);
            let inner = Frame::enter(true, scope, 3, EventKind::FdRetired, 1);
            let disk_error = std::io::Error::from_raw_os_error(28);
            observe(
                CoreError::storage_io(&"💾".repeat(65), &disk_error),
                Refusal::Io,
            );
            inner.finish(&Err::<(), _>(Refusal::Io));
            outer.finish(&Err::<(), _>(Refusal::Io));

            let mismatch = Frame::enter(true, scope, 4, EventKind::FdRetired, 0);
            observe(
                CoreError::Storage("wrong mapping".into()),
                Refusal::Capacity,
            );
            mismatch.finish(&Err::<(), _>(Refusal::Io));
            Frame::enter(true, scope, 5, EventKind::FdRetired, 0)
                .finish(&Err::<(), _>(Refusal::Io));
            let success = Frame::enter(true, scope, 6, EventKind::FdRetired, 0);
            observe(CoreError::Storage("success".into()), Refusal::Io);
            success.finish(&Ok::<(), Refusal>(()));
        });
        require(lines.len() == 4, "unexpected Io observation count")?;
        let rows: Vec<serde_json::Value> = lines
            .iter()
            .map(|line| serde_json::from_slice(line))
            .collect::<Result<_, _>>()?;
        let shm = rows.first().ok_or("missing SHM row")?;
        require(shm["scope_id"] == 23, "SHM scope differs")?;
        require(shm["handle_id"] == 3, "SHM handle differs")?;
        require(shm["callback_kind"] == "FdRetired", "SHM kind differs")?;
        require(shm["operation"] == 1, "SHM operation differs")?;
        require(shm["core_variant"] == "StorageIo", "SHM variant differs")?;
        require(shm["raw_errno"] == 28, "SHM errno differs")?;
        require(shm["mapped_refusal"] == "Io", "SHM mapping differs")?;
        require(shm["callback_returned"] == "Io", "SHM result differs")?;
        require(shm["detail_truncated"] == true, "SHM truncation missing")?;
        require(
            shm["detail"]
                .as_str()
                .is_some_and(|detail| detail.len() == 256 && detail.is_char_boundary(256)),
            "SHM detail is not bounded at UTF-8 boundary",
        )?;
        let main = rows.get(1).ok_or("missing main row")?;
        require(main["handle_id"] == 2, "main handle differs")?;
        require(main["operation"] == 0, "main operation differs")?;
        require(main["core_variant"] == "Storage", "main variant differs")?;
        require(main["detail"] == "outer", "outer observation overwritten")?;
        require(main["raw_errno"].is_null(), "invented main errno")?;
        for row in rows.iter().skip(2) {
            require(
                row["origin"] == "io_without_core_error",
                "direct Io origin differs",
            )?;
            require(row["core_variant"].is_null(), "unattributed core variant")?;
            require(row["detail"].is_null(), "unattributed core detail")?;
            require(row["mapped_refusal"].is_null(), "unattributed mapping")?;
            require(row["raw_errno"].is_null(), "invented direct errno")?;
        }
        Ok(())
    }

    #[test]
    fn distinct_mapped_errors_keep_the_matching_callback_observation() -> TestResult {
        let scope = NonZeroU64::new(31).ok_or("nonzero scope")?;
        let ((), lines) = capture_test_lines(|| {
            let capacity = Frame::enter(true, scope, 1, EventKind::BeforeWrite, 0);
            observe(CoreError::Storage("earlier Io".into()), Refusal::Io);
            observe(
                CoreError::Typed {
                    code: SearchPlaneErrorCodeV2::IngestResourceBudgetExceeded,
                    message: "budget detail".into(),
                },
                Refusal::Capacity,
            );
            capacity.finish(&Err::<(), _>(Refusal::Capacity));

            let io = Frame::enter(true, scope, 2, EventKind::FdRetired, 0);
            observe(
                CoreError::Typed {
                    code: SearchPlaneErrorCodeV2::IngestResourceBudgetExceeded,
                    message: "not the returned refusal".into(),
                },
                Refusal::Capacity,
            );
            observe(CoreError::Storage("actual Io".into()), Refusal::Io);
            io.finish(&Err::<(), _>(Refusal::Io));
        });
        require(lines.len() == 2, "mapped observations missing")?;
        let capacity: serde_json::Value = serde_json::from_slice(&lines[0])?;
        let io: serde_json::Value = serde_json::from_slice(&lines[1])?;
        require(
            capacity["detail"] == "budget detail",
            "budget detail was lost",
        )?;
        require(
            capacity["mapped_refusal"] == "Capacity",
            "budget mapping differs",
        )?;
        require(io["detail"] == "actual Io", "Io detail was lost")?;
        require(io["core_variant"] == "Storage", "Io variant differs")?;
        Ok(())
    }

    #[test]
    fn io_and_capacity_share_the_bounded_sample_pool() -> TestResult {
        let scope = NonZeroU64::new(29).ok_or("nonzero scope")?;
        let ((), lines) = capture_test_lines(|| {
            for handle in 1..=10 {
                let result = if handle % 2 == 0 {
                    Refusal::Capacity
                } else {
                    Refusal::Io
                };
                Frame::enter(true, scope, handle, EventKind::FdRetired, 0)
                    .finish(&Err::<(), _>(result));
            }
        });
        require(
            lines.len() == SAMPLE_LIMIT,
            "mixed refusal sampling exceeded bound",
        )?;
        Ok(())
    }

    #[test]
    fn observer_write_failure_does_not_change_callback_refusal() -> TestResult {
        let scope = NonZeroU64::new(17).ok_or("nonzero scope")?;
        let (result, lines) = capture_test_lines(|| {
            TEST_WRITE_FAILURE.with(|slot| slot.set(true));
            let frame = Frame::enter(true, scope, 1, EventKind::BeforeWrite, 0);
            observe(
                CoreError::Typed {
                    code: SearchPlaneErrorCodeV2::IngestResourceBudgetExceeded,
                    message: "capacity".into(),
                },
                Refusal::Capacity,
            );
            let result = Err::<(), _>(Refusal::Capacity);
            frame.finish(&result);
            let io_frame = Frame::enter(true, scope, 2, EventKind::FdRetired, 1);
            observe(CoreError::Storage("io".into()), Refusal::Io);
            let io_result = Err::<(), _>(Refusal::Io);
            io_frame.finish(&io_result);
            TEST_WRITE_FAILURE.with(|slot| slot.set(false));
            (result, io_result)
        });
        require(
            result == (Err(Refusal::Capacity), Err(Refusal::Io)),
            "writer failure changed callback results",
        )?;
        require(lines.is_empty(), "failed writer emitted a line")?;
        Ok(())
    }

    #[test]
    fn sample_limit_is_eight_and_thread_local_capture_is_isolated() -> TestResult {
        let scope = NonZeroU64::new(19).ok_or("nonzero scope")?;
        let child = std::thread::spawn(move || {
            capture_test_lines(|| {
                Frame::enter(true, scope, 41, EventKind::BeforeOpen, 0)
                    .finish(&Err::<(), _>(Refusal::Capacity));
            })
            .1
        });
        let ((), lines) = capture_test_lines(|| {
            for handle in 1..=10 {
                Frame::enter(true, scope, handle, EventKind::BeforeOpen, 0)
                    .finish(&Err::<(), _>(Refusal::Capacity));
            }
        });
        let child_lines = child
            .join()
            .map_err(|payload| std::io::Error::other(format!("child panicked: {payload:?}")))?;
        require(lines.len() == SAMPLE_LIMIT, "capture exceeded eight lines")?;
        require(child_lines.len() == 1, "child capture not isolated")?;
        let last: serde_json::Value =
            serde_json::from_slice(lines.get(7).ok_or("missing eighth line")?)?;
        let child_row: serde_json::Value =
            serde_json::from_slice(child_lines.first().ok_or("missing child line")?)?;
        require(
            last.get("sample_ordinal")
                .and_then(serde_json::Value::as_u64)
                == Some(8),
            "eighth ordinal differs",
        )?;
        require(
            child_row
                .get("sample_ordinal")
                .and_then(serde_json::Value::as_u64)
                == Some(1),
            "child ordinal differs",
        )?;
        require(
            child_row
                .get("handle_id")
                .and_then(serde_json::Value::as_u64)
                == Some(41),
            "child handle differs",
        )?;
        Ok(())
    }

    #[test]
    fn unwind_restores_outer_frame_without_stale_inner_observation() -> TestResult {
        let scope = NonZeroU64::new(21).ok_or("nonzero scope")?;
        let (result, lines) = capture_test_lines(|| {
            let outer = Frame::enter(true, scope, 1, EventKind::BeforeWrite, 0);
            let failed = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                let _inner = Frame::enter(true, scope, 2, EventKind::BeforeWrite, 0);
                observe(
                    CoreError::Typed {
                        code: SearchPlaneErrorCodeV2::IngestResourceBudgetExceeded,
                        message: "inner only".into(),
                    },
                    Refusal::Capacity,
                );
                std::panic::resume_unwind(Box::new("test unwind"));
            }));
            if failed.is_ok() {
                return Err(std::io::Error::other("inner frame did not unwind"));
            }
            observe(
                CoreError::Typed {
                    code: SearchPlaneErrorCodeV2::IngestResourceBudgetExceeded,
                    message: "outer only".into(),
                },
                Refusal::Capacity,
            );
            outer.finish(&Err::<(), _>(Refusal::Capacity));
            Ok(())
        });
        result?;
        require(lines.len() == 1, "unwind leaked inner observation")?;
        let row: serde_json::Value =
            serde_json::from_slice(lines.first().ok_or("missing outer line")?)?;
        require(
            row.get("detail").and_then(serde_json::Value::as_str) == Some("outer only"),
            "stale inner detail",
        )?;
        require(
            row.get("handle_id").and_then(serde_json::Value::as_u64) == Some(1),
            "outer handle differs",
        )?;
        Ok(())
    }
}
