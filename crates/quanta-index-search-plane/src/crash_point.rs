//! Named crash points of the seal and the physical GC that follows it
//! (QI-BB-029 완료 기준 #2, QI-BB-003 완료 기준 #2).
//!
//! The daemon-level crash matrix starts the daemon with
//! [`CRASH_POINT_ENV`] naming one point, lets a seal reach it, and
//! restarts the daemon to prove what survived and that a retried seal
//! converges on the same generation. The process exits at the point with
//! [`CRASH_EXIT_CODE`] — no unwinding, no destructors — as a crash would.
//!
//! Compiled into test and debug builds only; a release build has no crash
//! points and reads no environment here.

/// The environment variable naming the crash point to exit at.
pub const CRASH_POINT_ENV: &str = "QUANTA_INDEX_CRASH_POINT";

/// The exit code a crash point ends the process with.
pub const CRASH_EXIT_CODE: i32 = 86;

/// Every crash point, in protocol order: the seal's, then the GC's.
pub const ALL: [&str; 8] = [
    AFTER_SEMANTIC_SEAL,
    BEFORE_AUTHORITY_RECORD,
    AFTER_RETENTION_RECEIPT,
    AFTER_CATALOG_TRANSACTION,
    AFTER_LEDGER_RECONCILE,
    AFTER_FENCE,
    BETWEEN_TRACK_RECLAIMS,
    BEFORE_RECORD_FORGET,
];

/// The semantic track of a sealed batch is sealed; the lexical track is
/// not built.
pub const AFTER_SEMANTIC_SEAL: &str = "after_semantic_seal";
/// Both tracks are sealed and proven; the durable authority has not
/// recorded the generation.
pub const BEFORE_AUTHORITY_RECORD: &str = "before_authority_record";
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

/// Exit the process when [`CRASH_POINT_ENV`] names `point`.
#[cfg(any(test, debug_assertions))]
#[expect(
    clippy::exit,
    reason = "a crash point ends the process on purpose, without unwinding, exactly as a crash would; only test and debug builds carry it"
)]
pub(crate) fn reached(point: &str) {
    if let Ok(named) = std::env::var(CRASH_POINT_ENV)
        && named == point
    {
        std::process::exit(CRASH_EXIT_CODE);
    }
}

/// A release build has no crash points.
#[cfg(not(any(test, debug_assertions)))]
pub(crate) const fn reached(_point: &str) {}
