#![forbid(unsafe_code)]

#[cfg(all(test, target_os = "linux"))]
mod protected_read_tests;

#[cfg(test)]
mod tests {
    use std::{collections::BTreeMap, error::Error, fs, path::Path};

    use rusqlite::Connection;

    type TestResult = Result<(), Box<dyn Error>>;

    fn rows(connection: &Connection) -> rusqlite::Result<Vec<(i64, i64)>> {
        let mut statement = connection.prepare("SELECT n,length(payload) FROM gold ORDER BY n")?;
        statement
            .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?
            .collect()
    }

    fn wal_images(path: &Path, page_size: usize) -> Result<BTreeMap<u32, Vec<u8>>, Box<dyn Error>> {
        let wal = fs::read(path)?;
        let header = wal.get(..32).ok_or("missing WAL header")?;
        let encoded_size = u32::from_be_bytes(header[8..12].try_into()?);
        if usize::try_from(encoded_size)? != page_size {
            return Err("wrong WAL page size".into());
        }
        let frame_size = page_size.checked_add(24).ok_or("frame size overflow")?;
        let frames = &wal[32..];
        if frames.is_empty() || frames.len() % frame_size != 0 {
            return Err("incomplete WAL frames".into());
        }
        let mut images = BTreeMap::new();
        let mut committed = false;
        for frame in frames.chunks_exact(frame_size) {
            let page = u32::from_be_bytes(frame[0..4].try_into()?);
            if page == 0 {
                return Err("WAL page zero".into());
            }
            committed |= u32::from_be_bytes(frame[4..8].try_into()?) != 0;
            images.insert(page, frame[24..].to_vec());
        }
        if !committed {
            return Err("WAL has no commit marker".into());
        }
        Ok(images)
    }

    #[test]
    fn wal_snapshot_checkpoint_and_reopen_match_committed_pages() -> TestResult {
        for page_size in [1024_usize, 4096, 8192] {
            checkpoint_for_page_size(page_size)?;
        }
        Ok(())
    }

    fn checkpoint_for_page_size(page_size: usize) -> TestResult {
        let directory = tempfile::tempdir()?;
        let database = directory.path().join("upstream.sqlite");
        let initial = Connection::open(&database)?;
        initial.execute_batch(&format!(
            "PRAGMA page_size={page_size}; CREATE TABLE gold(n INTEGER PRIMARY KEY,payload BLOB);"
        ))?;
        drop(initial);

        let writer = Connection::open(&database)?;
        writer.execute_batch("PRAGMA journal_mode=WAL; PRAGMA synchronous=FULL; PRAGMA wal_autocheckpoint=0; PRAGMA wal_checkpoint(TRUNCATE);")?;
        let actual_page_size: i64 = writer.query_row("PRAGMA page_size", [], |row| row.get(0))?;
        if usize::try_from(actual_page_size)? != page_size {
            return Err("page size differs".into());
        }
        let old_reader = Connection::open(&database)?;
        old_reader.execute_batch("BEGIN;")?;
        if !rows(&old_reader)?.is_empty() {
            return Err("old snapshot not empty".into());
        }
        writer.execute_batch(
            "BEGIN IMMEDIATE; INSERT INTO gold VALUES(7,zeroblob(262144)); COMMIT;",
        )?;
        if !rows(&old_reader)?.is_empty() || rows(&writer)? != vec![(7, 262144)] {
            return Err("WAL snapshot or commit row differs".into());
        }
        let images = wal_images(&database.with_file_name("upstream.sqlite-wal"), page_size)?;
        if images.len() <= 8 {
            return Err("fixture has fewer than eight WAL pages".into());
        }

        let held_checkpoint: (i64, i64, i64) =
            writer.query_row("PRAGMA wal_checkpoint(PASSIVE)", [], |row| {
                Ok((row.get(0)?, row.get(1)?, row.get(2)?))
            })?;
        if held_checkpoint.0 != 0 || held_checkpoint.1 <= 0 || held_checkpoint.2 != 0 {
            return Err(
                format!("old snapshot permitted checkpoint backfill: {held_checkpoint:?}").into(),
            );
        }
        if !rows(&old_reader)?.is_empty() || rows(&writer)? != vec![(7, 262144)] {
            return Err("checkpoint changed a held snapshot or committed row".into());
        }

        old_reader.execute_batch("COMMIT;")?;
        drop(old_reader);
        let checkpoint: (i64, i64, i64) =
            writer.query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |row| {
                Ok((row.get(0)?, row.get(1)?, row.get(2)?))
            })?;
        if checkpoint != (0, 0, 0) {
            return Err("truncate checkpoint incomplete".into());
        }
        let main = fs::read(&database)?;
        for (page, image) in images {
            let start = (usize::try_from(page)? - 1)
                .checked_mul(page_size)
                .ok_or("page offset overflow")?;
            let end = start.checked_add(page_size).ok_or("page end overflow")?;
            if main.get(start..end) != Some(image.as_slice()) {
                return Err("checkpoint page differs from WAL image".into());
            }
        }
        drop(writer);
        let reopened = Connection::open(&database)?;
        if rows(&reopened)? != vec![(7, 262144)] {
            return Err("reopen row differs".into());
        }
        let integrity: String =
            reopened.query_row("PRAGMA integrity_check", [], |row| row.get(0))?;
        if integrity != "ok" {
            return Err(format!("integrity_check: {integrity}").into());
        }
        Ok(())
    }
}
