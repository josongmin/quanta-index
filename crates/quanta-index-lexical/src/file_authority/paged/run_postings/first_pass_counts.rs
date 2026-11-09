//! Bounded producer-only gram counts. Seal verification never consumes this cache.

use super::{CoreError, RecordEdit, RunGramCount, RunPostingPage, corrupt, invalid};

pub(super) const MAX_BYTES: u64 = 4 * 1024 * 1024;
// Keep the original conservative page-scratch reservation within the count
// cache ceiling even though the producer now supplies its validated page.
const PAGE_SCRATCH_RESERVE_BYTES: usize = 4 * super::MAX_POSTING_PAGE_BYTES;
type CountRow = ([u8; 4], u64);

pub(super) struct FirstPassCounts {
    rows: Option<Vec<CountRow>>,
    path_rows: u64,
    content_rows: u64,
}

impl FirstPassCounts {
    pub(super) fn new(byte_limit: u64) -> Self {
        let overhead = std::mem::size_of::<Self>() + PAGE_SCRATCH_RESERVE_BYTES;
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

    pub(super) fn observe_page(&mut self, page: &RunPostingPage) -> Result<(), CoreError> {
        let Some(rows) = self.rows.as_mut() else {
            return Ok(());
        };
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

#[cfg(test)]
mod tests {
    use super::super::RunPostingRow;
    use super::*;

    fn page(gram: [u8; 3], id: u64) -> RunPostingPage {
        RunPostingPage::new(
            1,
            gram,
            id >> 8,
            vec![RunPostingRow {
                logical_id: id,
                segment_sha256: [7; 32],
            }],
        )
        .expect("page")
    }

    #[test]
    fn bounded_count_cache_overflow_releases_acceleration_without_new_refusal() {
        let limit = u64::try_from(
            std::mem::size_of::<FirstPassCounts>()
                + PAGE_SCRATCH_RESERVE_BYTES
                + std::mem::size_of::<CountRow>(),
        )
        .expect("limit");
        let mut cache = FirstPassCounts::new(limit);
        cache.observe_page(&page(*b"abc", 1)).expect("first gram");
        cache
            .observe_page(&page(*b"abd", 2))
            .expect("overflow uses original path");
        assert!(cache.into_records().is_none());
        assert!(FirstPassCounts::new(0).into_records().is_none());
    }

    #[test]
    fn first_pass_counts_rejects_out_of_order_typed_pages() {
        let mut cache = FirstPassCounts::new(MAX_BYTES);
        cache.observe_page(&page(*b"abd", 1)).expect("first gram");
        assert!(cache.observe_page(&page(*b"abc", 2)).is_err());
    }
}
