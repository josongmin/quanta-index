//! Named crash points of the physical GC protocol (QI-BB-003 완료 기준 #2).
//!
//! The daemon-level crash matrix starts the daemon with
//! [`GC_CRASH_POINT_ENV`] naming one point, lets a seal reach it, and
//! restarts the daemon to prove what survived: the generations the durable
//! retention receipt retains are whole and served, the retired one is
//! unknown, and a retried seal finishes the GC. The process exits at the
//! point with [`GC_CRASH_EXIT_CODE`] — no unwinding, no destructors — as a
//! crash would.
//!
//! Compiled into test and debug builds only; a release build has no crash
//! points and reads no environment here.

/// The environment variable naming the crash point to exit at.
pub const GC_CRASH_POINT_ENV: &str = "QUANTA_INDEX_GC_CRASH_POINT";

/// The exit code a crash point ends the process with.
pub const GC_CRASH_EXIT_CODE: i32 = 86;

/// The protocol steps a crash can interrupt, in protocol order.
pub const GC_CRASH_POINTS: [&str; 6] = [
    AFTER_RETENTION_RECEIPT,
    AFTER_CATALOG_TRANSACTION,
    AFTER_LEDGER_RECONCILE,
    AFTER_FENCE,
    BETWEEN_TRACK_RECLAIMS,
    BEFORE_RECORD_FORGET,
];

/// The retention receipt is durable; nothing else of the seal's GC ran.
pub const AFTER_RETENTION_RECEIPT: &str = "after_retention_receipt";
/// The chunk universe and the auxiliary reap are one durable transaction;
/// the ledger has not seen them.
pub const AFTER_CATALOG_TRANSACTION: &str = "after_catalog_transaction";
/// The ledger has reaped the retired generations; nothing was fenced or
/// removed.
pub const AFTER_LEDGER_RECONCILE: &str = "after_ledger_reconcile";
/// A retired generation's snapshot handle is fenced; its directory is
/// still on disk.
pub const AFTER_FENCE: &str = "after_fence";
/// The lexical track's retired generations are removed; the semantic
/// track's are not.
pub const BETWEEN_TRACK_RECLAIMS: &str = "between_track_reclaims";
/// Every retired directory is removed; their idempotency records are not
/// forgotten.
pub const BEFORE_RECORD_FORGET: &str = "before_record_forget";

/// Exit the process when [`GC_CRASH_POINT_ENV`] names `point`.
#[cfg(any(test, debug_assertions))]
#[expect(
    clippy::exit,
    reason = "a crash point ends the process on purpose, without unwinding, exactly as a crash would; only test and debug builds carry it"
)]
pub(crate) fn crash_point(point: &str) {
    if let Ok(named) = std::env::var(GC_CRASH_POINT_ENV)
        && named == point
    {
        std::process::exit(GC_CRASH_EXIT_CODE);
    }
}

/// A release build has no crash points.
#[cfg(not(any(test, debug_assertions)))]
pub(crate) const fn crash_point(_point: &str) {}
