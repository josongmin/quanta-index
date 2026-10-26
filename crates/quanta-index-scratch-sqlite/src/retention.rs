//! Disposable request-local inode membership for exact retained-generation unions.
//!
//! The `SQLite` main file has an explicit page cap. Its memory journal, cache,
//! temporary query state, and filesystem allocation overhead are not an
//! aggregate RSS or disk reservation. The caller owns the generation lifecycle
//! guard and validates sealed identity before reusing a cached present row.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use quanta_index_contract::ManifestGeneration;
use quanta_index_core::domains::reclaim_area::{
    RegularFileInodeV1, visit_regular_inode_tree_below_track,
    visit_regular_inode_tree_below_track_with_budget,
};
use quanta_index_core::{CoreError, DiagnosticContextV1, RequestBudgetV1, SealedGenerationBytesV1};
use rusqlite::{Connection, OptionalExtension as _, Transaction, params};
use tempfile::TempDir;

const PAGE_BYTES: u64 = 4096;

fn storage(context: &str, error: impl std::fmt::Display) -> CoreError {
    CoreError::Storage(format!("retained inode membership {context}: {error}"))
}

fn sql_error(context: &str, error: &rusqlite::Error) -> CoreError {
    // Engine FULL/NOMEM does not prove an application reservation was refused.
    storage(context, error)
}

fn decode_u64(blob: &[u8]) -> Result<u64, CoreError> {
    let bytes: [u8; 8] = blob.try_into().map_err(|error| {
        CoreError::Storage(format!(
            "retained inode membership invalid u64 blob: {error}"
        ))
    })?;
    Ok(u64::from_be_bytes(bytes))
}

/// Request-local membership cache. Dropping it removes the disposable DB.
pub struct RetainedInodeMembership {
    db: Connection,
    _directory: TempDir,
    selected: BTreeSet<ManifestGeneration>,
    selected_bytes: u64,
}

impl RetainedInodeMembership {
    /// Open a private DB with an explicit maximum main-file size.
    ///
    /// `max_db_bytes` is rounded down to whole 4096-byte pages. It does not
    /// reserve capacity or cap `SQLite`'s process memory and memory journal.
    pub fn open(max_db_bytes: u64) -> Result<Self, CoreError> {
        let max_pages = max_db_bytes.checked_div(PAGE_BYTES).ok_or_else(|| {
            CoreError::InvalidContract("retained inode membership page size is zero".into())
        })?;
        if max_pages == 0 || max_pages > u64::from(u32::MAX.saturating_sub(1)) {
            return Err(CoreError::InvalidContract(
                "retained inode membership max_db_bytes outside SQLite page range".into(),
            ));
        }
        let directory = tempfile::tempdir().map_err(|error| storage("tempdir", error))?;
        let db_path = directory.path().join("retained-inodes.sqlite");
        let db = Connection::open(&db_path).map_err(|error| sql_error("open", &error))?;
        db.execute_batch(
            "PRAGMA page_size=4096;
             PRAGMA journal_mode=MEMORY;
             PRAGMA temp_store=MEMORY;
             PRAGMA synchronous=OFF;
             PRAGMA mmap_size=0;",
        )
        .map_err(|error| sql_error("configure", &error))?;
        let page_size: i64 = db
            .query_row("PRAGMA main.page_size", [], |row| row.get(0))
            .map_err(|error| sql_error("page size", &error))?;
        let journal_mode: String = db
            .query_row("PRAGMA main.journal_mode", [], |row| row.get(0))
            .map_err(|error| sql_error("journal mode", &error))?;
        let temp_store: i64 = db
            .query_row("PRAGMA temp_store", [], |row| row.get(0))
            .map_err(|error| sql_error("temp store", &error))?;
        let page_size = u64::try_from(page_size).map_err(|error| {
            CoreError::Storage(format!(
                "retained inode membership SQLite page size invalid: {error}"
            ))
        })?;
        if page_size != PAGE_BYTES || journal_mode != "memory" || temp_store != 2 {
            return Err(CoreError::Storage(
                "retained inode membership SQLite bounded configuration refused".into(),
            ));
        }
        let cache_kib = max_db_bytes
            .checked_div(1024)
            .ok_or_else(|| {
                CoreError::InvalidContract("retained inode membership KiB divisor is zero".into())
            })?
            .clamp(4, 2048);
        db.execute_batch(&format!("PRAGMA cache_size=-{cache_kib};"))
            .map_err(|error| sql_error("cache cap", &error))?;
        let installed: i64 = db
            .query_row(
                &format!("PRAGMA main.max_page_count={max_pages}"),
                [],
                |row| row.get(0),
            )
            .map_err(|error| sql_error("main page cap", &error))?;
        let installed = u64::try_from(installed).map_err(|error| {
            CoreError::Storage(format!(
                "retained inode membership SQLite main page cap invalid: {error}"
            ))
        })?;
        if installed != max_pages {
            return Err(CoreError::Storage(
                "retained inode membership SQLite main page cap refused".into(),
            ));
        }
        db.execute_batch(
            "CREATE TABLE present_generation (
               generation BLOB PRIMARY KEY CHECK(length(generation)=8)
             ) WITHOUT ROWID;
             CREATE TABLE generation_binding (
               generation BLOB PRIMARY KEY CHECK(length(generation)=8),
               commitment BLOB NOT NULL CHECK(length(commitment)=32)
             ) WITHOUT ROWID;
             CREATE TABLE inode (
               device BLOB NOT NULL CHECK(length(device)=8),
               inode BLOB NOT NULL CHECK(length(inode)=8),
               bytes BLOB NOT NULL CHECK(length(bytes)=8),
               PRIMARY KEY(device,inode)
             ) WITHOUT ROWID;
             CREATE TABLE membership (
               device BLOB NOT NULL CHECK(length(device)=8),
               inode BLOB NOT NULL CHECK(length(inode)=8),
               generation BLOB NOT NULL CHECK(length(generation)=8),
               PRIMARY KEY(device,inode,generation)
             ) WITHOUT ROWID;
             CREATE INDEX membership_by_generation
               ON membership(generation,device,inode);
             CREATE TABLE active_inode (
               device BLOB NOT NULL CHECK(length(device)=8),
               inode BLOB NOT NULL CHECK(length(inode)=8),
               bytes BLOB NOT NULL CHECK(length(bytes)=8),
               refcount BLOB NOT NULL CHECK(length(refcount)=8),
               PRIMARY KEY(device,inode)
             ) WITHOUT ROWID;",
        )
        .map_err(|error| sql_error("schema", &error))?;
        Ok(Self {
            db,
            _directory: directory,
            selected: BTreeSet::new(),
            selected_bytes: 0,
        })
    }

    /// Whether this request has already completed a scan of a present generation.
    /// A missing generation is never cached and must be probed again.
    pub fn contains_generation(&self, generation: ManifestGeneration) -> Result<bool, CoreError> {
        self.db
            .query_row(
                "SELECT 1 FROM present_generation WHERE generation=?1",
                [&generation.get().to_be_bytes()[..]],
                |_row| Ok(()),
            )
            .optional()
            .map(|value| value.is_some())
            .map_err(|error| sql_error("lookup generation", &error))
    }

    /// Bind a generation to the caller-authenticated sealed commitment.
    /// A later different commitment is refused without replacing the first.
    pub fn bind_generation(
        &self,
        generation: ManifestGeneration,
        commitment: [u8; 32],
    ) -> Result<(), CoreError> {
        let tx = self
            .db
            .unchecked_transaction()
            .map_err(|error| sql_error("begin generation binding", &error))?;
        let generation_bytes = generation.get().to_be_bytes();
        let _inserted_binding = tx
            .execute(
                "INSERT OR IGNORE INTO generation_binding(generation,commitment) VALUES (?1,?2)",
                params![&generation_bytes[..], &commitment[..]],
            )
            .map_err(|error| sql_error("record generation binding", &error))?;
        let recorded: Vec<u8> = tx
            .query_row(
                "SELECT commitment FROM generation_binding WHERE generation=?1",
                [&generation_bytes[..]],
                |row| row.get(0),
            )
            .map_err(|error| sql_error("read generation binding", &error))?;
        if recorded.as_slice() != &commitment[..] {
            return Err(CoreError::InvalidContract(
                "retained inode membership generation commitment changed".into(),
            ));
        }
        tx.commit()
            .map_err(|error| sql_error("commit generation binding", &error))?;
        Ok(())
    }

    fn cache_generation_with(
        &mut self,
        generation: ManifestGeneration,
        walk: impl FnOnce(
            &mut dyn FnMut(RegularFileInodeV1) -> Result<(), CoreError>,
        ) -> Result<bool, CoreError>,
    ) -> Result<bool, CoreError> {
        if self.contains_generation(generation)? {
            return Ok(true);
        }
        let tx = self
            .db
            .transaction()
            .map_err(|error| sql_error("begin scan", &error))?;
        let present = {
            let mut record = |file| record_inode(&tx, generation, file);
            walk(&mut record)?
        };
        if !present {
            return Ok(false);
        }
        let _inserted_generation = tx
            .execute(
                "INSERT INTO present_generation(generation) VALUES (?1)",
                [&generation.get().to_be_bytes()[..]],
            )
            .map_err(|error| sql_error("record generation", &error))?;
        tx.commit()
            .map_err(|error| sql_error("commit scan", &error))?;
        Ok(true)
    }

    /// Measure the exact union of requested generations using authenticated
    /// per-generation scans. The caller must hold a guard that prevents a
    /// cached present generation from changing and validate its sealed identity.
    /// `path_for` must return a path one or two components below `track_root`.
    pub fn measure(
        &mut self,
        track_root: &Path,
        requested: &BTreeSet<ManifestGeneration>,
        path_for: &dyn Fn(ManifestGeneration) -> PathBuf,
        skip: &dyn Fn(&str) -> bool,
        diagnostic: Option<&DiagnosticContextV1>,
    ) -> Result<SealedGenerationBytesV1, CoreError> {
        self.measure_inner(track_root, requested, path_for, skip, None, diagnostic)
    }

    /// Use one caller-owned absolute deadline for every probe in a pair plan.
    pub fn measure_with_budget(
        &mut self,
        track_root: &Path,
        requested: &BTreeSet<ManifestGeneration>,
        path_for: &dyn Fn(ManifestGeneration) -> PathBuf,
        skip: &dyn Fn(&str) -> bool,
        budget: &RequestBudgetV1,
        diagnostic: Option<&DiagnosticContextV1>,
    ) -> Result<SealedGenerationBytesV1, CoreError> {
        self.measure_inner(
            track_root,
            requested,
            path_for,
            skip,
            Some(budget),
            diagnostic,
        )
    }

    fn measure_inner(
        &mut self,
        track_root: &Path,
        requested: &BTreeSet<ManifestGeneration>,
        path_for: &dyn Fn(ManifestGeneration) -> PathBuf,
        skip: &dyn Fn(&str) -> bool,
        budget: Option<&RequestBudgetV1>,
        diagnostic: Option<&DiagnosticContextV1>,
    ) -> Result<SealedGenerationBytesV1, CoreError> {
        if let Some(budget) = budget {
            budget.checkpoint("retention-inodes:entry")?;
        }
        let mut absent = BTreeSet::new();
        for &generation in requested {
            if let Some(budget) = budget {
                budget.checkpoint("retention-inodes:generation")?;
            }
            let path = path_for(generation);
            if !self.cache_generation_with(generation, |visitor| {
                if let Some(budget) = budget {
                    visit_regular_inode_tree_below_track_with_budget(
                        track_root, &path, skip, budget, diagnostic, visitor,
                    )
                } else {
                    visit_regular_inode_tree_below_track(
                        track_root, &path, skip, diagnostic, visitor,
                    )
                }
            })? {
                let _new_absence = absent.insert(generation);
            }
        }
        let present: BTreeSet<_> = requested.difference(&absent).copied().collect();
        let removed: Vec<_> = self.selected.difference(&present).copied().collect();
        let added: Vec<_> = present.difference(&self.selected).copied().collect();
        if !removed.is_empty() || !added.is_empty() {
            let tx = self
                .db
                .transaction()
                .map_err(|error| sql_error("begin union", &error))?;
            let mut total = self.selected_bytes;
            // Removing first avoids an intermediate overflow when the final
            // requested union fits after replacing one selected generation.
            for &generation in &removed {
                apply_generation(&tx, generation, false, &mut total, budget)?;
            }
            for &generation in &added {
                apply_generation(&tx, generation, true, &mut total, budget)?;
            }
            if let Some(budget) = budget {
                budget.checkpoint("retention-inodes:commit")?;
            }
            tx.commit()
                .map_err(|error| sql_error("commit union", &error))?;
            self.selected = present;
            self.selected_bytes = total;
        }
        Ok(SealedGenerationBytesV1 {
            bytes: self.selected_bytes,
            absent,
        })
    }
}

fn apply_generation(
    tx: &Transaction<'_>,
    generation: ManifestGeneration,
    add: bool,
    total: &mut u64,
    budget: Option<&RequestBudgetV1>,
) -> Result<(), CoreError> {
    let mut statement = tx
        .prepare(
            "SELECT m.device,m.inode,i.bytes
             FROM membership AS m LEFT JOIN inode AS i
               ON i.device=m.device AND i.inode=m.inode
             WHERE m.generation=?1 ORDER BY m.device,m.inode",
        )
        .map_err(|error| sql_error("prepare generation members", &error))?;
    let generation_bytes = generation.get().to_be_bytes();
    let mut rows = statement
        .query([&generation_bytes[..]])
        .map_err(|error| sql_error("query generation members", &error))?;
    while let Some(row) = rows
        .next()
        .map_err(|error| sql_error("read generation member", &error))?
    {
        if let Some(budget) = budget {
            budget.checkpoint("retention-inodes:union-row")?;
        }
        let device: Vec<u8> = row
            .get(0)
            .map_err(|error| sql_error("read device", &error))?;
        let inode: Vec<u8> = row
            .get(1)
            .map_err(|error| sql_error("read inode", &error))?;
        let bytes: Option<Vec<u8>> = row
            .get(2)
            .map_err(|error| sql_error("read bytes", &error))?;
        let file_bytes = decode_u64(&bytes.ok_or_else(|| {
            CoreError::Storage("retained inode membership row lacks inode".into())
        })?)?;
        let key = params![&device[..], &inode[..]];
        let active: Option<(Vec<u8>, Vec<u8>)> = tx
            .query_row(
                "SELECT bytes,refcount FROM active_inode WHERE device=?1 AND inode=?2",
                key,
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()
            .map_err(|error| sql_error("read active inode", &error))?;
        match (add, active) {
            (true, None) => {
                *total = total.checked_add(file_bytes).ok_or_else(|| {
                    CoreError::Storage("retained inode membership union byte overflow".into())
                })?;
                let _inserted_active = tx.execute(
                    "INSERT INTO active_inode(device,inode,bytes,refcount) VALUES (?1,?2,?3,?4)",
                    params![
                        &device[..],
                        &inode[..],
                        &file_bytes.to_be_bytes()[..],
                        &1_u64.to_be_bytes()[..]
                    ],
                )
                .map_err(|error| sql_error("activate inode", &error))?;
            }
            (true, Some((prior_bytes, count))) => {
                if decode_u64(&prior_bytes)? != file_bytes {
                    return Err(CoreError::Storage(
                        "retained inode membership active inode length conflict".into(),
                    ));
                }
                let count = decode_u64(&count)?;
                if count == 0 {
                    return Err(CoreError::Storage(
                        "retained inode membership zero active refcount".into(),
                    ));
                }
                let next = count.checked_add(1).ok_or_else(|| {
                    CoreError::Storage("retained inode membership refcount overflow".into())
                })?;
                let _updated_refcount = tx
                    .execute(
                        "UPDATE active_inode SET refcount=?3 WHERE device=?1 AND inode=?2",
                        params![&device[..], &inode[..], &next.to_be_bytes()[..]],
                    )
                    .map_err(|error| sql_error("increment inode refcount", &error))?;
            }
            (false, Some((prior_bytes, count))) => {
                if decode_u64(&prior_bytes)? != file_bytes {
                    return Err(CoreError::Storage(
                        "retained inode membership active inode length conflict".into(),
                    ));
                }
                let count = decode_u64(&count)?;
                if count == 0 {
                    return Err(CoreError::Storage(
                        "retained inode membership zero active refcount".into(),
                    ));
                }
                if count == 1 {
                    *total = total.checked_sub(file_bytes).ok_or_else(|| {
                        CoreError::Storage("retained inode membership union byte underflow".into())
                    })?;
                    let _deleted_active = tx
                        .execute(
                            "DELETE FROM active_inode WHERE device=?1 AND inode=?2",
                            params![&device[..], &inode[..]],
                        )
                        .map_err(|error| sql_error("deactivate inode", &error))?;
                } else {
                    let next = count.checked_sub(1).ok_or_else(|| {
                        CoreError::Storage("retained inode membership refcount underflow".into())
                    })?;
                    let _updated_refcount = tx
                        .execute(
                            "UPDATE active_inode SET refcount=?3 WHERE device=?1 AND inode=?2",
                            params![&device[..], &inode[..], &next.to_be_bytes()[..]],
                        )
                        .map_err(|error| sql_error("decrement inode refcount", &error))?;
                }
            }
            (false, None) => {
                return Err(CoreError::Storage(
                    "retained inode membership selected inode missing".into(),
                ));
            }
        }
    }
    Ok(())
}

fn record_inode(
    tx: &Transaction<'_>,
    generation: ManifestGeneration,
    file: RegularFileInodeV1,
) -> Result<(), CoreError> {
    let device = file.device.to_be_bytes();
    let inode = file.inode.to_be_bytes();
    let bytes = file.bytes.to_be_bytes();
    let _inserted_inode = tx
        .execute(
            "INSERT OR IGNORE INTO inode(device,inode,bytes) VALUES (?1,?2,?3)",
            params![&device[..], &inode[..], &bytes[..]],
        )
        .map_err(|error| sql_error("record inode", &error))?;
    let prior: Vec<u8> = tx
        .query_row(
            "SELECT bytes FROM inode WHERE device=?1 AND inode=?2",
            params![&device[..], &inode[..]],
            |row| row.get(0),
        )
        .map_err(|error| sql_error("read inode", &error))?;
    if decode_u64(&prior)? != file.bytes {
        return Err(CoreError::Storage(
            "retained inode membership conflicting inode length".into(),
        ));
    }
    let _inserted_membership = tx
        .execute(
            "INSERT OR IGNORE INTO membership(device,inode,generation) VALUES (?1,?2,?3)",
            params![&device[..], &inode[..], &generation.get().to_be_bytes()[..]],
        )
        .map_err(|error| sql_error("record membership", &error))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use quanta_index_contract::SearchPlaneErrorCodeV2;
    use std::fs;

    #[test]
    fn unscoped_full_nomem_and_io_errors_are_not_retention_admission() {
        for code in [
            rusqlite::ffi::SQLITE_FULL,
            rusqlite::ffi::SQLITE_NOMEM,
            rusqlite::ffi::SQLITE_IOERR_WRITE,
            rusqlite::ffi::SQLITE_IOERR_FSYNC,
        ] {
            let error = rusqlite::Error::SqliteFailure(rusqlite::ffi::Error::new(code), None);
            assert!(matches!(sql_error("write", &error), CoreError::Storage(_)));
        }
    }

    fn requested(values: &[u64]) -> BTreeSet<ManifestGeneration> {
        values
            .iter()
            .copied()
            .map(ManifestGeneration::new)
            .collect()
    }

    fn measure(
        membership: &mut RetainedInodeMembership,
        track: &Path,
        values: &[u64],
    ) -> Result<SealedGenerationBytesV1, CoreError> {
        membership.measure(
            track,
            &requested(values),
            &|generation| track.join(format!("gen-{}", generation.get())),
            &|_name| false,
            None,
        )
    }

    #[test]
    fn hard_link_is_charged_once_for_any_requested_subset() {
        let temp = tempfile::tempdir().unwrap();
        let track = temp.path().join("track");
        fs::create_dir_all(track.join("gen-1")).unwrap();
        fs::create_dir(track.join("gen-2")).unwrap();
        fs::write(track.join("gen-1/shared"), b"0123456789").unwrap();
        fs::hard_link(track.join("gen-1/shared"), track.join("gen-2/shared")).unwrap();
        fs::write(track.join("gen-2/private"), b"1234567").unwrap();
        let mut cache = RetainedInodeMembership::open(1024 * 1024).unwrap();
        assert_eq!(measure(&mut cache, &track, &[1]).unwrap().bytes, 10);
        assert_eq!(measure(&mut cache, &track, &[2]).unwrap().bytes, 17);
        assert_eq!(measure(&mut cache, &track, &[1, 2]).unwrap().bytes, 17);
        assert_eq!(measure(&mut cache, &track, &[1]).unwrap().bytes, 10);
        assert_eq!(measure(&mut cache, &track, &[2]).unwrap().bytes, 17);
        assert_eq!(measure(&mut cache, &track, &[2, 1]).unwrap().bytes, 17);
        assert_eq!(measure(&mut cache, &track, &[]).unwrap().bytes, 0);
    }

    #[test]
    fn expired_pair_budget_refuses_without_caching_a_partial_generation() {
        let temp = tempfile::tempdir().unwrap();
        let track = temp.path().join("track");
        fs::create_dir_all(track.join("gen-1")).unwrap();
        fs::write(track.join("gen-1/one"), b"12345").unwrap();
        fs::write(track.join("gen-1/two"), b"1234567").unwrap();
        let mut cache = RetainedInodeMembership::open(1024 * 1024).unwrap();
        let requested = requested(&[1]);
        let exhausted = RequestBudgetV1::unbounded();
        exhausted.cancel_handle().cancel();
        assert!(matches!(
            cache.measure_with_budget(
                &track,
                &requested,
                &|generation| track.join(format!("gen-{}", generation.get())),
                &|_| false,
                &exhausted,
                None,
            ),
            Err(CoreError::Typed {
                code: SearchPlaneErrorCodeV2::RequestCancelled,
                ..
            })
        ));
        assert!(
            !cache
                .contains_generation(ManifestGeneration::new(1))
                .unwrap()
        );
        assert_eq!(
            cache
                .measure_with_budget(
                    &track,
                    &requested,
                    &|generation| track.join(format!("gen-{}", generation.get())),
                    &|_| false,
                    &RequestBudgetV1::unbounded(),
                    None,
                )
                .unwrap()
                .bytes,
            12,
        );
    }

    #[test]
    fn missing_generation_is_reprobed() {
        let temp = tempfile::tempdir().unwrap();
        let track = temp.path().join("track");
        fs::create_dir(&track).unwrap();
        let mut cache = RetainedInodeMembership::open(1024 * 1024).unwrap();
        let absent = measure(&mut cache, &track, &[3]).unwrap();
        assert_eq!(absent.bytes, 0);
        assert_eq!(absent.absent, requested(&[3]));
        assert!(
            !cache
                .contains_generation(ManifestGeneration::new(3))
                .unwrap()
        );
        fs::create_dir(track.join("gen-3")).unwrap();
        fs::write(track.join("gen-3/file"), b"abcd").unwrap();
        let present = measure(&mut cache, &track, &[3]).unwrap();
        assert_eq!(present.bytes, 4);
        assert!(present.absent.is_empty());
        assert!(
            cache
                .contains_generation(ManifestGeneration::new(3))
                .unwrap()
        );
    }

    #[test]
    fn generation_binding_is_set_once() {
        let cache = RetainedInodeMembership::open(1024 * 1024).unwrap();
        let generation = ManifestGeneration::new(9);
        assert!(!cache.contains_generation(generation).unwrap());
        cache.bind_generation(generation, [7; 32]).unwrap();
        cache.bind_generation(generation, [7; 32]).unwrap();
        assert!(matches!(
            cache.bind_generation(generation, [8; 32]),
            Err(CoreError::InvalidContract(_))
        ));
        cache.bind_generation(generation, [7; 32]).unwrap();
        assert!(!cache.contains_generation(generation).unwrap());
    }

    #[test]
    fn sqlite_main_cap_refuses_scan_and_retry_uses_rolled_back_state() {
        let mut cache = RetainedInodeMembership::open(PAGE_BYTES * 16).unwrap();
        let generation = ManifestGeneration::new(11);
        let error = cache
            .cache_generation_with(generation, |visit| {
                for inode in 0..10_000_u64 {
                    visit(RegularFileInodeV1 {
                        device: 1,
                        inode,
                        bytes: 1,
                    })?;
                }
                Ok(true)
            })
            .unwrap_err();
        assert!(matches!(error, CoreError::Storage(_)));
        assert!(!cache.contains_generation(generation).unwrap());
        assert!(
            cache
                .cache_generation_with(generation, |visit| {
                    visit(RegularFileInodeV1 {
                        device: 1,
                        inode: 0,
                        bytes: 7,
                    })?;
                    Ok(true)
                })
                .unwrap()
        );
        assert_eq!(
            measure(&mut cache, Path::new("/unused-track"), &[11])
                .unwrap()
                .bytes,
            7
        );
    }

    #[test]
    fn failed_scan_rolls_back_and_can_retry_without_partial_membership() {
        let mut cache = RetainedInodeMembership::open(1024 * 1024).unwrap();
        let generation = ManifestGeneration::new(5);
        let file = RegularFileInodeV1 {
            device: 1,
            inode: 2,
            bytes: 5,
        };
        assert!(
            cache
                .cache_generation_with(generation, |visit| {
                    visit(file)?;
                    Err(CoreError::Storage("injected walk failure".into()))
                })
                .is_err()
        );
        assert!(!cache.contains_generation(generation).unwrap());
        assert!(
            cache
                .cache_generation_with(generation, |visit| {
                    visit(RegularFileInodeV1 { bytes: 7, ..file })?;
                    Ok(true)
                })
                .unwrap()
        );
        let track = Path::new("/nonexistent-retention-test-track");
        assert_eq!(measure(&mut cache, track, &[5]).unwrap().bytes, 7);
    }

    #[test]
    fn conflicting_inode_lengths_and_union_overflow_refuse() {
        let mut cache = RetainedInodeMembership::open(1024 * 1024).unwrap();
        let first = RegularFileInodeV1 {
            device: u64::MAX,
            inode: u64::MAX,
            bytes: u64::MAX,
        };
        assert!(
            cache
                .cache_generation_with(ManifestGeneration::new(1), |visit| {
                    visit(first)?;
                    Ok(true)
                })
                .unwrap()
        );
        assert!(
            cache
                .cache_generation_with(ManifestGeneration::new(2), |visit| {
                    visit(RegularFileInodeV1 { bytes: 1, ..first })?;
                    Ok(true)
                })
                .is_err()
        );
        assert!(
            !cache
                .contains_generation(ManifestGeneration::new(2))
                .unwrap()
        );
        assert!(
            cache
                .cache_generation_with(ManifestGeneration::new(2), |visit| {
                    visit(RegularFileInodeV1 {
                        device: 1,
                        inode: 1,
                        bytes: 0,
                    })?;
                    visit(RegularFileInodeV1 {
                        device: 2,
                        inode: 2,
                        bytes: 1,
                    })?;
                    Ok(true)
                })
                .unwrap()
        );
        let track = Path::new("/nonexistent-retention-test-track");
        assert_eq!(measure(&mut cache, track, &[1]).unwrap().bytes, u64::MAX);
        assert!(measure(&mut cache, track, &[1, 2]).is_err());
        // The first generation-2 row activated before overflow. The failed
        // union must roll back that row and retain the prior selected set.
        assert_eq!(measure(&mut cache, track, &[1]).unwrap().bytes, u64::MAX);
        assert_eq!(measure(&mut cache, track, &[2]).unwrap().bytes, 1);
    }
}
