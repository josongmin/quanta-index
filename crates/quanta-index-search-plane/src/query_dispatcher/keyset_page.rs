//! Bounded keyset page selection shared by the runtime-metadata and
//! structural routes (QI-BB-025 W4).
//!
//! A page walk names its position with the total-order key of the last
//! row it returned; the next page is the `top_k` smallest keys strictly
//! after it. [`KeysetPageCollector`] selects that page from any stream of
//! keys, in any order, keeping at most `top_k + 1` of them — the page and
//! one continuation probe — in a max-heap, so memory is proportional to
//! the page and never to the stream, and each key costs `O(log top_k)`.
//! The cursor is a boundary, not a lookup: a key that names no row of the
//! stream still positions the page correctly.
//!
//! What the collector counts is honest about what it saw: `examined` is
//! every key the caller visited, `matched` every key after the cursor it
//! was offered. A caller whose stream is in key order may stop as soon as
//! the collector is full, since no later key can enter the page; it then
//! finishes the page as [`StreamEnd::Stopped`] and the window reports the
//! matched count as a lower bound. A caller that must see every key —
//! the structural route, whose window is exact — finishes as
//! [`StreamEnd::Exhausted`].

use std::collections::BinaryHeap;

use quanta_index_contract::{CandidateCountV1, QueryResultWindowV1, continuation_fetch_size};
use quanta_index_core::{CoreError, validate_query_top_k};

/// Whether the caller offered the collector every key of its stream.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum StreamEnd {
    /// Every key was offered: the matched count is exact.
    Exhausted,
    /// The caller stopped once the page and its probe were full: the
    /// matched count is a lower bound. Valid only for a stream in key
    /// order, where nothing after the stop could have entered the page.
    Stopped,
}

/// Keeps the `top_k + 1` smallest keys after the cursor of everything
/// offered, and counts what it examined and what matched.
pub(super) struct KeysetPageCollector<K> {
    /// The page's rows.
    page_rows: usize,
    /// The page plus its continuation probe: what the heap may hold.
    limit: usize,
    after: Option<K>,
    /// Max-heap: the largest kept key is the one a smaller key evicts.
    kept: BinaryHeap<K>,
    examined: u64,
    matched: u64,
    /// The most keys the heap held at once, for the memory oracle.
    peak_retained: usize,
}

/// One selected page: its keys in order, its window, and what positions
/// the next page.
#[derive(Debug)]
pub(super) struct KeysetPage<K> {
    /// At most `top_k` keys, ascending.
    pub(super) keys: Vec<K>,
    pub(super) window: QueryResultWindowV1,
    pub(super) examined: u64,
    /// The last key on the page, when a next page exists.
    pub(super) next_key: Option<K>,
}

impl<K> KeysetPageCollector<K>
where
    K: Ord + Clone,
{
    /// A collector for a page of `top_k` rows strictly after `after`.
    ///
    /// `top_k` is validated against the public range here so the fetch
    /// size (`top_k + 1`) is the contract's continuation probe.
    pub(super) fn new(top_k: u32, after: Option<K>) -> Result<Self, CoreError> {
        let accepted = validate_query_top_k(top_k)?;
        let page_rows = usize::try_from(accepted).map_err(|error| {
            CoreError::InvalidContract(format!("keyset page: top_k overflows usize: {error}"))
        })?;
        let limit = usize::try_from(continuation_fetch_size(accepted)).map_err(|error| {
            CoreError::InvalidContract(format!(
                "keyset page: continuation fetch size overflows usize: {error}"
            ))
        })?;
        Ok(Self {
            page_rows,
            limit,
            after,
            kept: BinaryHeap::with_capacity(limit),
            examined: 0,
            matched: 0,
            peak_retained: 0,
        })
    }

    /// Count one visited key, matching or not.
    pub(super) fn examined_one(&mut self) {
        self.examined = self.examined.saturating_add(1);
    }

    /// Offer a matching key. Keys at or before the cursor belong to an
    /// earlier page and are ignored; a key that cannot enter the page is
    /// counted and dropped without being cloned.
    pub(super) fn offer(&mut self, key: &K) {
        if self.after.as_ref().is_some_and(|after| key <= after) {
            return;
        }
        self.matched = self.matched.saturating_add(1);
        if self.kept.len() < self.limit {
            self.kept.push(key.clone());
        } else if self.kept.peek().is_some_and(|largest| key < largest) {
            // The largest kept key leaves; the heap never exceeds `limit`.
            drop(self.kept.pop());
            self.kept.push(key.clone());
        }
        self.peak_retained = self.peak_retained.max(self.kept.len());
    }

    /// Whether the page and its continuation probe are both filled. A
    /// caller whose stream is in key order may stop here.
    pub(super) fn is_full(&self) -> bool {
        self.kept.len() >= self.limit
    }

    /// The most keys the collector held at once: the memory oracle the
    /// unit tests hold the collector to.
    #[cfg(test)]
    pub(super) const fn peak_retained(&self) -> usize {
        self.peak_retained
    }

    /// The page in key order, its window and its continuation.
    ///
    /// The window counts the matches after the cursor: exactly when the
    /// stream was exhausted, as a lower bound when the caller stopped at
    /// a full probe. A stopped stream with no probe row is a caller
    /// defect and is refused rather than reported as a page.
    pub(super) fn finish(self, end: StreamEnd) -> Result<KeysetPage<K>, CoreError> {
        let mut keys = self.kept.into_sorted_vec();
        let has_more = keys.len() > self.page_rows;
        keys.truncate(self.page_rows);
        let returned = u32::try_from(keys.len()).map_err(|error| {
            CoreError::Storage(format!("keyset page: row count overflows u32: {error}"))
        })?;
        let candidate_count = match end {
            StreamEnd::Exhausted => CandidateCountV1::Exact(self.matched),
            StreamEnd::Stopped => CandidateCountV1::AtLeast(self.matched),
        };
        let window = QueryResultWindowV1::new(returned, candidate_count, has_more)
            .map_err(|error| CoreError::Storage(format!("keyset page: result window: {error}")))?;
        let next_key = if has_more { keys.last().cloned() } else { None };
        Ok(KeysetPage {
            keys,
            window,
            examined: self.examined,
            next_key,
        })
    }
}

#[cfg(test)]
mod tests {
    use quanta_index_contract::CandidateCountV1;
    use quanta_index_core::CoreError;

    use super::{KeysetPageCollector, StreamEnd};

    type TestRes = Result<(), Box<dyn std::error::Error>>;

    /// Walk `keys` (offered in the order given) page by page of `top_k`
    /// until the collector reports no next page; every page finishes as
    /// `end`.
    fn walk(keys: &[u32], top_k: u32, end: StreamEnd) -> Result<Vec<Vec<u32>>, CoreError> {
        let mut pages = Vec::new();
        let mut cursor: Option<u32> = None;
        loop {
            let mut collector = KeysetPageCollector::new(top_k, cursor)?;
            for key in keys {
                collector.examined_one();
                collector.offer(key);
            }
            let page = collector.finish(end)?;
            let next = page.next_key;
            pages.push(page.keys);
            match next {
                Some(next) => cursor = Some(next),
                None => return Ok(pages),
            }
        }
    }

    #[test]
    fn pages_partition_an_unordered_stream_in_key_order_without_gaps_or_overlap() -> TestRes {
        // 23 keys in a scrambled order: the stream order must not matter.
        let keys: Vec<u32> = (0..23u32).map(|i| (i.wrapping_mul(7)) % 23).collect();
        let pages = walk(&keys, 5, StreamEnd::Exhausted)?;
        let sizes: Vec<usize> = pages.iter().map(Vec::len).collect();
        if sizes != [5, 5, 5, 5, 3] {
            return Err(format!("23 keys page by 5 as 5/5/5/5/3, got {sizes:?}").into());
        }
        let walked: Vec<u32> = pages.into_iter().flatten().collect();
        let expected: Vec<u32> = (0..23u32).collect();
        if walked != expected {
            return Err(format!("the walk is the keys in order, once each: {walked:?}").into());
        }
        Ok(())
    }

    #[test]
    fn a_forged_cursor_is_a_boundary_not_a_lookup() -> TestRes {
        let keys: Vec<u32> = vec![10, 20, 30, 40, 50];
        // 25 is no key of the stream; the page after it is 30, 40, 50.
        let mut collector = KeysetPageCollector::new(2, Some(25))?;
        for key in &keys {
            collector.examined_one();
            collector.offer(key);
        }
        let page = collector.finish(StreamEnd::Exhausted)?;
        if page.keys != [30, 40] || page.next_key != Some(40) {
            return Err(format!("the page after a forged cursor: {page:?}").into());
        }
        if page.window.candidate_count() != CandidateCountV1::Exact(3) || !page.window.has_more() {
            return Err(format!("three keys follow the boundary: {:?}", page.window).into());
        }
        if page.examined != 5 {
            return Err(format!("every key was examined, got {}", page.examined).into());
        }
        Ok(())
    }

    #[test]
    fn the_collector_retains_at_most_the_page_and_its_probe() -> TestRes {
        const ROWS: u32 = 10_000;
        const TOP_K: u32 = 5;
        // Descending order is the worst case for a max-heap of the
        // smallest keys: every key qualifies until the heap is full, and
        // every later key evicts the current maximum.
        let mut collector = KeysetPageCollector::new(TOP_K, None)?;
        for key in (0..ROWS).rev() {
            collector.examined_one();
            collector.offer(&key);
        }
        let peak = collector.peak_retained();
        let page = collector.finish(StreamEnd::Exhausted)?;
        let bound = usize::try_from(TOP_K)?.saturating_add(1);
        if peak > bound {
            return Err(format!("peak retained {peak} exceeds top_k + 1 = {bound}").into());
        }
        if page.keys != [0, 1, 2, 3, 4] || page.next_key != Some(4) {
            return Err(format!("the five smallest keys are the page: {page:?}").into());
        }
        if page.window.candidate_count() != CandidateCountV1::Exact(u64::from(ROWS)) {
            return Err(format!("every row matched: {:?}", page.window).into());
        }
        if page.examined != u64::from(ROWS) {
            return Err(format!("every row was examined, got {}", page.examined).into());
        }
        Ok(())
    }

    #[test]
    fn an_ordered_stream_may_stop_at_a_full_probe_and_reports_a_lower_bound() -> TestRes {
        let mut collector = KeysetPageCollector::new(3, None)?;
        let mut visited = 0u64;
        for key in 0..100u32 {
            collector.examined_one();
            visited = visited.saturating_add(1);
            collector.offer(&key);
            if collector.is_full() {
                break;
            }
        }
        if visited != 4 {
            return Err(format!("the walk stops after the probe row, visited {visited}").into());
        }
        let page = collector.finish(StreamEnd::Stopped)?;
        if page.keys != [0, 1, 2]
            || page.next_key != Some(2)
            || page.window.candidate_count() != CandidateCountV1::AtLeast(4)
            || !page.window.has_more()
            || page.examined != 4
        {
            return Err(format!("a stopped walk is a probe window: {page:?}").into());
        }
        Ok(())
    }

    #[test]
    fn an_exhausted_stream_within_the_page_is_exact_with_no_continuation() -> TestRes {
        let mut collector = KeysetPageCollector::new(5, None)?;
        for key in [3u32, 1, 2] {
            collector.examined_one();
            collector.offer(&key);
        }
        let page = collector.finish(StreamEnd::Exhausted)?;
        if page.keys != [1, 2, 3]
            || page.next_key.is_some()
            || page.window.candidate_count() != CandidateCountV1::Exact(3)
            || page.window.has_more()
        {
            return Err(format!("a final page: {page:?}").into());
        }
        Ok(())
    }

    #[test]
    fn a_stopped_stream_without_a_probe_row_is_refused_not_reported() -> TestRes {
        let mut collector = KeysetPageCollector::new(5, None)?;
        collector.offer(&1u32);
        match collector.finish(StreamEnd::Stopped) {
            Err(CoreError::Storage(message)) if message.contains("result window") => Ok(()),
            other => {
                Err(format!("a lower bound with no continuation is refused: {other:?}").into())
            }
        }
    }

    #[test]
    fn keys_at_or_before_the_cursor_neither_match_nor_are_kept() -> TestRes {
        let mut collector = KeysetPageCollector::new(2, Some(5u32))?;
        for key in [1u32, 5, 6, 7] {
            collector.examined_one();
            collector.offer(&key);
        }
        let page = collector.finish(StreamEnd::Exhausted)?;
        if page.keys != [6, 7]
            || page.window.candidate_count() != CandidateCountV1::Exact(2)
            || page.window.has_more()
            || page.examined != 4
        {
            return Err(format!("only keys after the cursor match: {page:?}").into());
        }
        Ok(())
    }

    #[test]
    fn top_k_is_validated_against_the_public_range() {
        assert!(KeysetPageCollector::<u32>::new(0, None).is_err());
        assert!(KeysetPageCollector::<u32>::new(1, None).is_ok());
    }
}
