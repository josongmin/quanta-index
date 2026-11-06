//! Bounded producer-only gram counts. Seal verification never consumes this cache.

use super::{CoreError, RecordEdit, RunGramCount, RunPostingPage, corrupt, invalid};

pub(super) const MAX_BYTES: u64 = 4 * 1024 * 1024;
// Includes the transient decoded posting page and its wire palette/rows.
const DECODE_SCRATCH_BYTES: usize = 4 * super::MAX_POSTING_PAGE_BYTES;
type CountRow = ([u8; 4], u64);

pub(super) struct FirstPassCounts {
    rows: Option<Vec<CountRow>>,
    path_rows: u64,
    content_rows: u64,
}

impl FirstPassCounts {
    pub(super) fn new(byte_limit: u64) -> Self {
        let overhead = std::mem::size_of::<Self>() + DECODE_SCRATCH_BYTES;
        let limit = usize::try_from(byte_limit.min(MAX_BYTES)).unwrap_or(0);
        let capacity = limit.saturating_sub(overhead) / std::mem::size_of::<CountRow>();
        let mut rows = Vec::new();
        let enabled =
            capacity > 0 && rows.try_reserve_exact(capacity).is_ok() && rows.capacity() <= capacity;
        Self {
            rows: enabled.then_some(rows),
            path_rows: 0,
            content_rows: 0,
        }
    }

    fn observe(&mut self, edit: &RecordEdit) -> Result<(), CoreError> {
        let Some(rows) = self.rows.as_mut() else {
            return Ok(());
        };
        let page = RunPostingPage::decode(&edit.bytes)?;
        if page.key()?.as_slice() != edit.key {
            return Err(corrupt("first-pass page key differs"));
        }
        let key = [page.surface, page.gram[0], page.gram[1], page.gram[2]];
        let count = u64::try_from(page.rows.len()).map_err(|_| invalid("page count width"))?;
        if let Some((previous, total)) = rows.last_mut() {
            if *previous > key {
                return Err(corrupt("first-pass gram order differs"));
            }
            if *previous == key {
                *total = total
                    .checked_add(count)
                    .ok_or_else(|| invalid("gram count overflow"))?;
            } else if rows.len() == rows.capacity() {
                self.rows = None;
                return Ok(());
            } else {
                rows.push((key, count));
            }
        } else {
            rows.push((key, count));
        }
        let total = if page.surface == 1 {
            &mut self.path_rows
        } else {
            &mut self.content_rows
        };
        *total = total
            .checked_add(count)
            .ok_or_else(|| invalid("posting total overflow"))?;
        Ok(())
    }

    pub(super) fn into_records(self) -> Option<(CountRecords, u64, u64)> {
        self.rows.map(|rows| {
            (
                CountRecords {
                    rows: rows.into_iter(),
                },
                self.path_rows,
                self.content_rows,
            )
        })
    }
}

pub(super) struct CountRecords {
    rows: std::vec::IntoIter<CountRow>,
}

impl Iterator for CountRecords {
    type Item = Result<RecordEdit, CoreError>;
    fn next(&mut self) -> Option<Self::Item> {
        self.rows.next().map(|(key, count)| {
            let bytes = RunGramCount {
                surface: key[0],
                gram: [key[1], key[2], key[3]],
                count,
            }
            .encode()?;
            Ok(RecordEdit {
                key: key.to_vec(),
                bytes,
            })
        })
    }
}

pub(super) struct CountingPages<'a, I> {
    source: I,
    counts: &'a mut FirstPassCounts,
    failed: bool,
}

impl<'a, I> CountingPages<'a, I> {
    pub(super) fn new(source: I, counts: &'a mut FirstPassCounts) -> Self {
        Self {
            source,
            counts,
            failed: false,
        }
    }
}

impl<I: Iterator<Item = Result<RecordEdit, CoreError>>> Iterator for CountingPages<'_, I> {
    type Item = Result<RecordEdit, CoreError>;
    fn next(&mut self) -> Option<Self::Item> {
        if self.failed {
            return None;
        }
        let result = self.source.next()?.and_then(|edit| {
            self.counts.observe(&edit)?;
            Ok(edit)
        });
        self.failed = result.is_err();
        Some(result)
    }
}

#[cfg(test)]
mod tests {
    use super::super::RunPostingRow;
    use super::*;

    fn edit(gram: [u8; 3], id: u64) -> RecordEdit {
        let page = RunPostingPage::new(
            1,
            gram,
            id >> 8,
            vec![RunPostingRow {
                logical_id: id,
                segment_sha256: [7; 32],
            }],
        )
        .expect("page");
        RecordEdit {
            key: page.key().expect("key").to_vec(),
            bytes: page.encode().expect("bytes"),
        }
    }

    #[test]
    fn bounded_count_cache_overflow_releases_acceleration_without_new_refusal() {
        let limit = u64::try_from(
            std::mem::size_of::<FirstPassCounts>()
                + DECODE_SCRATCH_BYTES
                + std::mem::size_of::<CountRow>(),
        )
        .expect("limit");
        let mut cache = FirstPassCounts::new(limit);
        cache.observe(&edit(*b"abc", 1)).expect("first gram");
        cache
            .observe(&edit(*b"abd", 2))
            .expect("overflow uses original path");
        assert!(cache.into_records().is_none());
        assert!(FirstPassCounts::new(0).into_records().is_none());
    }

    #[test]
    fn first_pass_counts_rejects_malformed_posting_and_wrong_key() {
        let mut cache = FirstPassCounts::new(MAX_BYTES);
        assert!(
            cache
                .observe(&RecordEdit {
                    key: vec![0; 12],
                    bytes: vec![255]
                })
                .is_err()
        );
        let mut wrong = edit(*b"abc", 1);
        wrong.key[1] = b'z';
        assert!(cache.observe(&wrong).is_err());
    }
}
