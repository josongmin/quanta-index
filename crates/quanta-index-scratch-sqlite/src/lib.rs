//! Shared admission for disposable `SQLite` main databases.
//!
//! The main-file page cap is enforced by `SQLite` after shared reservation succeeds.
//! This does not account for `SQLite` secondary temporary files, memory, or filesystem
//! allocation overhead. Those require separate writer-level enforcement.
//!
//! Explicit reservation refusals keep their budget outcome. Unscoped `SQLite`
//! FULL/NOMEM/IO errors remain storage failures: a main-page cap alone cannot
//! distinguish its own guardrail from native disk or allocator exhaustion.

mod connection;
mod edit_page;
mod edit_write;
pub use edit_page::{MapEditPage, read_map_edit_budgeted};
pub use edit_write::write_map_edit_batch;
pub mod physical;
pub mod retention;
pub use connection::{ScratchConnection, ScratchConnectionScope, storage_error};

use quanta_index_core::index::{
    IndexError, MapValue, OrdinalMapEdit, ScratchAllocation, WorkBudget,
};
use rusqlite::{Connection, Row, types::ValueRef};

const PAGE_BYTES: u64 = 4096;
const GROWTH_BYTES: u64 = 256 * 1024;
const WRITE_HEADROOM: u64 = 64 * 1024;
pub const MAX_WRITE_BATCH_ROWS: usize = 64;

/// Borrow a `SQLite` BLOB after checking its bound, before an owned Rust copy.
/// This does not constrain `SQLite`'s own allocation of the row or page cache.
pub fn bounded_blob(value: ValueRef<'_>, max_bytes: usize) -> Result<&[u8], IndexError> {
    let ValueRef::Blob(bytes) = value else {
        return Err(IndexError::CorruptObject(
            "scratch value is not a BLOB".into(),
        ));
    };
    if bytes.len() > max_bytes {
        return Err(IndexError::CorruptObject(
            "scratch BLOB exceeds its field bound".into(),
        ));
    }
    Ok(bytes)
}

/// Decode an exact-width scratch field onto the stack, without a temporary `Vec`.
pub fn fixed_blob<const N: usize>(value: ValueRef<'_>) -> Result<[u8; N], IndexError> {
    bounded_blob(value, N)?.try_into().map_err(|_error| {
        IndexError::CorruptObject("scratch BLOB width differs from its field".into())
    })
}

/// Bind canonical bounded map values without an unadmitted heap encoding.
#[derive(Clone, Copy)]
pub struct SqlMapValue(quanta_index_core::index::EncodedMapValue);
impl rusqlite::ToSql for SqlMapValue {
    fn to_sql(&self) -> rusqlite::Result<rusqlite::types::ToSqlOutput<'_>> {
        self.0
            .as_slice()
            .map(|bytes| rusqlite::types::ToSqlOutput::Borrowed(ValueRef::Blob(bytes)))
            .map_err(|error| rusqlite::Error::ToSqlConversionFailure(Box::new(error)))
    }
}
pub fn map_value(value: &MapValue) -> Result<SqlMapValue, IndexError> {
    Ok(SqlMapValue(value.encoded()?))
}
pub fn optional_map_value(value: ValueRef<'_>) -> Result<Option<MapValue>, IndexError> {
    if value == ValueRef::Null {
        Ok(None)
    } else {
        MapValue::decode(bounded_blob(
            value,
            quanta_index_core::index::MAX_MAP_VALUE_WIRE_BYTES,
        )?)
        .map(Some)
    }
}

/// Decode the canonical (key, expected, next) scratch row into the existing IR.
///
/// Validate both tagged values before copying the bounded key. NULL denotes absence;
/// present bounded values must use their exact current wire encoding.
pub fn read_map_edit(
    row: &Row<'_>,
    max_key_bytes: usize,
    root: u32,
    budget: &WorkBudget,
) -> Result<OrdinalMapEdit, IndexError> {
    let key = bounded_blob(row.get_ref(0).map_err(|error| sql(&error))?, max_key_bytes)?;
    let expected = optional_map_value(row.get_ref(1).map_err(|error| sql(&error))?)?;
    let next = optional_map_value(row.get_ref(2).map_err(|error| sql(&error))?)?;
    OrdinalMapEdit::copy_key(root, key, expected, next, budget)
}

fn sql(error: &rusqlite::Error) -> IndexError {
    storage_error(error, "scratch main-file admission")
}

fn pragma(db: &Connection, statement: &str) -> Result<u64, IndexError> {
    let value: i64 = db
        .query_row(statement, [], |row| row.get(0))
        .map_err(|error| sql(&error))?;
    u64::try_from(value)
        .map_err(|_error| IndexError::Storage("negative scratch page counter".into()))
}

/// Install and verify a main-file cap using capacity already held by this allocation.
pub fn configure(db: &Connection, allocation: &dyn ScratchAllocation) -> Result<(), IndexError> {
    db.execute_batch("PRAGMA page_size=4096;")
        .map_err(|error| sql(&error))?;
    install_limit(db, allocation)
}

fn install_limit(db: &Connection, allocation: &dyn ScratchAllocation) -> Result<(), IndexError> {
    let reserved = allocation.reserved_bytes()?;
    if reserved > allocation.max_bytes() {
        return Err(IndexError::BackendViolation(
            "scratch reservation exceeds envelope".into(),
        ));
    }
    let pages = reserved
        .checked_div(PAGE_BYTES)
        .ok_or(IndexError::ResourceLimited)?;
    if pages == 0 {
        return Err(IndexError::ResourceLimited);
    }
    let page_size = pragma(db, "PRAGMA main.page_size")?;
    if page_size != PAGE_BYTES {
        return Err(IndexError::Storage(
            "scratch page size differs from admitted page size".into(),
        ));
    }
    let installed = pragma(db, &format!("PRAGMA main.max_page_count={pages}"))?;
    if installed != pages {
        return Err(IndexError::Storage(
            "scratch main-file limit was not installed".into(),
        ));
    }
    Ok(())
}

fn verify_limit(db: &Connection, allocation: &dyn ScratchAllocation) -> Result<(), IndexError> {
    if pragma(db, "PRAGMA main.page_size")? != PAGE_BYTES
        || pragma(db, "PRAGMA main.max_page_count")?
            .checked_mul(PAGE_BYTES)
            .ok_or(IndexError::ResourceLimited)?
            > allocation.reserved_bytes()?
    {
        return Err(IndexError::Storage(
            "scratch main-file page limit exceeds reservation".into(),
        ));
    }
    Ok(())
}

/// Admit capacity before a mutation.
///
/// Headroom predicts ordinary B-tree growth; `SQLite`'s installed hard cap remains authoritative if this estimate is too low.
/// A `SQLITE_FULL` error or any admission/configuration failure must abort the
/// attempt, never replay a partially rolled-back write. The connection is private;
/// callers must not change page limits or continue after a failed installation.
pub fn reserve_for_write(
    db: &Connection,
    allocation: &dyn ScratchAllocation,
    incoming_bytes: u64,
    budget: &mut WorkBudget,
) -> Result<(), IndexError> {
    budget.checkpoint()?;
    let pages = pragma(db, "PRAGMA main.page_count")?;
    let free = pragma(db, "PRAGMA main.freelist_count")?;
    let current = pages
        .checked_mul(PAGE_BYTES)
        .ok_or(IndexError::ResourceLimited)?;
    let live = pages
        .checked_sub(free)
        .and_then(|value| value.checked_mul(PAGE_BYTES))
        .ok_or(IndexError::ResourceLimited)?;
    let required = incoming_bytes
        .checked_mul(2)
        .and_then(|value| value.checked_add(WRITE_HEADROOM))
        .and_then(|value| value.checked_add(live))
        .ok_or(IndexError::ResourceLimited)?
        .max(current);
    let reserved = allocation.reserved_bytes()?;
    if required <= reserved {
        verify_limit(db, allocation)?;
        return Ok(());
    }
    if required > allocation.max_bytes() {
        return Err(IndexError::ResourceLimited);
    }
    let rounded = required
        .checked_add(
            GROWTH_BYTES
                .checked_sub(1)
                .ok_or(IndexError::ResourceLimited)?,
        )
        .and_then(|value| value.checked_div(GROWTH_BYTES))
        .and_then(|value| value.checked_mul(GROWTH_BYTES))
        .ok_or(IndexError::ResourceLimited)?
        .min(allocation.max_bytes());
    allocation.reserve_at_least(rounded, budget)?;
    // A failed PRAGMA retains the newly acquired reservation until cleanup.
    install_limit(db, allocation)
}

/// Admit one bounded group of writes before its first mutation.
///
/// Callers must abort the transaction on any admission or `SQLite` failure; a hard page cap
/// remains authoritative if B-tree growth exceeds this estimate.
pub fn reserve_for_write_batch(
    db: &Connection,
    allocation: &dyn ScratchAllocation,
    row_bytes: &[u64],
    budget: &mut WorkBudget,
) -> Result<(), IndexError> {
    if row_bytes.is_empty() || row_bytes.len() > MAX_WRITE_BATCH_ROWS {
        return Err(IndexError::ResourceLimited);
    }
    let incoming = row_bytes.iter().try_fold(0u64, |sum, row| {
        sum.checked_add(*row).ok_or(IndexError::ResourceLimited)
    })?;
    reserve_for_write(db, allocation, incoming, budget)
}

#[cfg(test)]
mod tests {
    use std::{
        path::Path,
        sync::{
            Arc,
            atomic::{AtomicU64, Ordering},
        },
        time::{Duration, Instant},
    };

    use quanta_index_core::index::{Digest, ObjectRef, WorkLimits};

    use super::*;

    macro_rules! check {
        ($condition:expr $(,)?) => {{
            if !$condition {
                return Err(IndexError::BackendViolation(format!(
                    "test assertion failed at {}:{}: {}", file!(), line!(), stringify!($condition)
                )).into());
            }
        }};
        ($condition:expr, $($message:tt)+) => {{
            if !$condition {
                return Err(IndexError::BackendViolation(format!($($message)+)).into());
            }
        }};
    }
    macro_rules! check_eq {
        ($left:expr, $right:expr $(,)?) => {{
            let left = &$left;
            let right = &$right;
            if *left != *right {
                return Err(IndexError::BackendViolation(format!(
                    "test equality failed at {}:{}: {left:?} != {right:?}",
                    file!(),
                    line!()
                ))
                .into());
            }
        }};
    }

    struct TestAllocation {
        directory: tempfile::TempDir,
        maximum: u64,
        reserved: AtomicU64,
        shared_reserved: Arc<AtomicU64>,
        refuse_growth: bool,
    }

    impl TestAllocation {
        fn new(initial: u64, maximum: u64, refuse_growth: bool) -> Result<Self, IndexError> {
            let directory = tempfile::tempdir()
                .map_err(|error| IndexError::Storage(format!("scratch test directory: {error}")))?;
            Ok(Self {
                directory,
                maximum,
                reserved: AtomicU64::new(initial),
                shared_reserved: Arc::new(AtomicU64::new(initial)),
                refuse_growth,
            })
        }

        fn open(&self) -> Result<Connection, IndexError> {
            Connection::open(self.path().join("test.sqlite")).map_err(|error| sql(&error))
        }

        fn shared_bytes(&self) -> u64 {
            self.shared_reserved.load(Ordering::Acquire)
        }
    }

    impl ScratchAllocation for TestAllocation {
        fn path(&self) -> &Path {
            self.directory.path()
        }

        fn max_bytes(&self) -> u64 {
            self.maximum
        }

        fn reserved_bytes(&self) -> Result<u64, IndexError> {
            Ok(self.reserved.load(Ordering::Acquire))
        }

        fn reserve_at_least(&self, bytes: u64, budget: &mut WorkBudget) -> Result<(), IndexError> {
            budget.checkpoint()?;
            if self.refuse_growth || bytes > self.maximum {
                return Err(IndexError::ResourceLimited);
            }
            let prior = self.reserved.load(Ordering::Acquire);
            if bytes > prior {
                let delta = bytes
                    .checked_sub(prior)
                    .ok_or(IndexError::ResourceLimited)?;
                let _old_total = self
                    .shared_reserved
                    .fetch_update(Ordering::AcqRel, Ordering::Acquire, |total| {
                        total.checked_add(delta)
                    })
                    .map_err(|_total| IndexError::ResourceLimited)?;
                self.reserved.store(bytes, Ordering::Release);
            }
            Ok(())
        }
    }

    fn budget() -> Result<WorkBudget, IndexError> {
        Ok(WorkBudget::new(
            WorkLimits {
                reads: 100,
                writes: 100,
                bytes: 1_000_000,
                mutations: 100,
            },
            Instant::now()
                .checked_add(Duration::from_secs(10))
                .ok_or(IndexError::ResourceLimited)?,
        ))
    }

    fn main_page_cap(db: &Connection) -> Result<u64, IndexError> {
        let pages: i64 = db
            .query_row("PRAGMA main.max_page_count", [], |row| row.get(0))
            .map_err(|error| sql(&error))?;
        u64::try_from(pages)
            .map_err(|_error| IndexError::Storage("negative scratch test page cap".into()))
    }

    #[test]
    fn denied_growth_does_not_raise_sqlite_cap_or_shared_reservation() -> Result<(), IndexError> {
        let allocation = TestAllocation::new(256 * 1024, 1024 * 1024, true)?;
        let db = allocation.open()?;
        configure(&db, &allocation)?;
        db.execute_batch("CREATE TABLE rows(value BLOB);")
            .map_err(|error| sql(&error))?;
        check_eq!(main_page_cap(&db)?, 64);
        check_eq!(allocation.shared_bytes(), 256 * 1024);

        check!(matches!(
            reserve_for_write(&db, &allocation, 200 * 1024, &mut budget()?),
            Err(IndexError::ResourceLimited)
        ));
        check_eq!(main_page_cap(&db)?, 64);
        check_eq!(allocation.reserved_bytes()?, 256 * 1024);
        check_eq!(allocation.shared_bytes(), 256 * 1024);
        Ok(())
    }

    #[test]
    fn batch_admission_is_bounded_and_preserves_hard_page_cap() -> Result<(), IndexError> {
        let allocation = TestAllocation::new(256 * 1024, 1024 * 1024, false)?;
        let db = allocation.open()?;
        configure(&db, &allocation)?;
        db.execute_batch("CREATE TABLE rows(value BLOB); BEGIN IMMEDIATE;")
            .map_err(|error| sql(&error))?;
        check!(matches!(
            reserve_for_write_batch(&db, &allocation, &[], &mut budget()?),
            Err(IndexError::ResourceLimited)
        ));
        check!(matches!(
            reserve_for_write_batch(
                &db,
                &allocation,
                &[1; MAX_WRITE_BATCH_ROWS + 1],
                &mut budget()?
            ),
            Err(IndexError::ResourceLimited)
        ));
        reserve_for_write_batch(
            &db,
            &allocation,
            &[1024; MAX_WRITE_BATCH_ROWS],
            &mut budget()?,
        )?;
        check!(
            main_page_cap(&db)?
                .checked_mul(PAGE_BYTES)
                .ok_or(IndexError::ResourceLimited)?
                <= allocation.reserved_bytes()?
        );
        check_eq!(allocation.shared_bytes(), allocation.reserved_bytes()?);

        // A bypassed main-file cap must be refused even when estimated
        // headroom fits the reservation; callers cannot continue the attempt.
        db.execute_batch("PRAGMA main.max_page_count=256;")
            .map_err(|error| sql(&error))?;
        check!(matches!(
            reserve_for_write_batch(&db, &allocation, &[1], &mut budget()?),
            Err(IndexError::Storage(_))
        ));
        Ok(())
    }

    #[test]
    fn configure_refuses_sub_page_reservation_before_setting_a_cap() -> Result<(), IndexError> {
        let allocation = TestAllocation::new(2048, 1024 * 1024, false)?;
        let db = allocation.open()?;
        check!(matches!(
            configure(&db, &allocation),
            Err(IndexError::ResourceLimited)
        ));
        check!(main_page_cap(&db)? > 1);
        check_eq!(allocation.reserved_bytes()?, 2048);
        check_eq!(allocation.shared_bytes(), 2048);
        Ok(())
    }

    #[test]
    fn failed_page_size_verification_retains_preacquired_reservation() -> Result<(), IndexError> {
        let allocation = TestAllocation::new(256 * 1024, 1024 * 1024, false)?;
        let db = allocation.open()?;
        db.execute_batch("PRAGMA page_size=8192; CREATE TABLE rows(value BLOB);")
            .map_err(|error| sql(&error))?;
        let page_size: u32 = db
            .query_row("PRAGMA main.page_size", [], |row| row.get(0))
            .map_err(|error| sql(&error))?;
        check_eq!(page_size, 8192);

        check!(matches!(
            reserve_for_write(&db, &allocation, 200 * 1024, &mut budget()?),
            Err(IndexError::Storage(_))
        ));
        check!(allocation.reserved_bytes()? > 256 * 1024);
        check_eq!(allocation.shared_bytes(), allocation.reserved_bytes()?);
        Ok(())
    }

    #[test]
    fn borrowed_blob_bounds_preserve_binary_bytes_and_refuse_other_sql_types()
    -> Result<(), IndexError> {
        let binary = [0, 0xff, 0x80, 7];
        let borrowed = bounded_blob(ValueRef::Blob(&binary), 4)?;
        check_eq!(borrowed, binary.as_slice());
        check!(std::ptr::eq(borrowed.as_ptr(), binary.as_ptr()));
        check!(matches!(
            bounded_blob(ValueRef::Blob(&binary), 3),
            Err(IndexError::CorruptObject(_))
        ));
        for value in [
            ValueRef::Null,
            ValueRef::Integer(1),
            ValueRef::Real(1.0),
            ValueRef::Text(b"blob"),
        ] {
            check!(matches!(
                bounded_blob(value, 4),
                Err(IndexError::CorruptObject(_))
            ));
        }
        Ok(())
    }

    #[test]
    fn fixed_blob_requires_exact_width_including_empty_and_large_values() -> Result<(), IndexError>
    {
        let bytes = [7; 32];
        check_eq!(fixed_blob::<32>(ValueRef::Blob(&bytes))?, bytes);
        let oversized = vec![7; 1_048_576];
        for value in [&[][..], &[7; 31][..], &[7; 33][..], oversized.as_slice()] {
            check!(matches!(
                fixed_blob::<32>(ValueRef::Blob(value)),
                Err(IndexError::CorruptObject(_))
            ));
        }
        check_eq!(
            fixed_blob::<3>(ValueRef::Blob(&[0, 0xff, 8]))?,
            [0, 0xff, 8]
        );
        Ok(())
    }

    #[test]
    fn map_edit_sql_decoder_preserves_null_and_exact_refs_and_refuses_corruption()
    -> Result<(), IndexError> {
        let work = budget()?;
        let db = Connection::open_in_memory().map_err(|error| sql(&error))?;
        let key = [0u8, 0xff, 8];
        let prior = MapValue::Object(ObjectRef(Digest([7; 32])));
        let next = MapValue::inline(&[8; 32], Some(ObjectRef(Digest([9; 32]))))?;
        for (expected, replacement) in [
            (None, Some(next)),
            (Some(prior), None),
            (Some(prior), Some(next)),
        ] {
            let edit = db
                .query_row(
                    "SELECT ?1,?2,?3",
                    rusqlite::params![
                        key,
                        expected.as_ref().map(map_value).transpose()?,
                        replacement.as_ref().map(map_value).transpose()?
                    ],
                    |row| Ok(read_map_edit(row, 3, 0, &work)),
                )
                .map_err(|error| sql(&error))??;
            check_eq!(edit.edit.key, key.to_vec());
            check_eq!(edit.edit.expected, expected);
            check_eq!(edit.edit.next, replacement);
        }
        for statement in [
            "SELECT zeroblob(4),NULL,zeroblob(32)",
            "SELECT x'01',zeroblob(31),NULL",
            "SELECT x'01',NULL,zeroblob(34)",
            "SELECT x'01',NULL,zeroblob(1048576)",
            "SELECT x'01',x'',NULL",
            "SELECT x'01',NULL,'not-a-blob'",
            "SELECT x'01',42,NULL",
        ] {
            let result = db
                .query_row(statement, [], |row| Ok(read_map_edit(row, 3, 0, &work)))
                .map_err(|error| sql(&error))?;
            check!(matches!(result, Err(IndexError::CorruptObject(_))));
        }
        Ok(())
    }

    mod fused_writes;
}
