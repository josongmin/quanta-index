//! The relevance page collector: bounded selection after a cursor, under
//! the search plane's row predicate, with an exact match count.
//!
//! Every document the compiled expression matches is visited once (no
//! block pruning: the count the page reports is exact, not a bound).
//! For each, the collector reads the fast columns that place it under
//! the relevance total order, drops it if it is at or before the cursor,
//! asks the predicate whether the row belongs on the page, counts it,
//! and keeps it only if it is among the `limit` best seen so far — so
//! the resident set is bounded by the page, not by the match set.

use std::collections::BinaryHeap;
use std::sync::Arc;

use quanta_index_contract::HistoryScoreV1;
use quanta_index_contract::lex::CommitSha;
use quanta_index_core::{
    CoreError, HistoryTextAdmitFn, HistoryTextDocKeyV1, HistoryTextHitV1, HistoryTextKindV1,
};
use tantivy::collector::{Collector, SegmentCollector};
use tantivy::columnar::{BytesColumn, Column, StrColumn};
use tantivy::{DocId, Score, SegmentOrdinal, SegmentReader};

use crate::history_text_index::schema::KindSchema;

/// What one collect produced.
pub(super) struct RelevanceFruit {
    /// Documents visited.
    pub(super) examined: u64,
    /// Documents after the cursor the predicate admitted.
    pub(super) matched: u64,
    /// The best `limit` admitted hits, in relevance order.
    pub(super) hits: Vec<HistoryTextHitV1>,
    /// The first error the collect met; the fruit is then not a page.
    pub(super) error: Option<CoreError>,
}

/// The relevance page collector for one kind's index.
pub(super) struct RelevanceCollector {
    kind: HistoryTextKindV1,
    limit: usize,
    after: Option<HistoryTextHitV1>,
    admit: Arc<HistoryTextAdmitFn>,
}

impl RelevanceCollector {
    pub(super) fn new(
        kind: HistoryTextKindV1,
        limit: usize,
        after: Option<HistoryTextHitV1>,
        admit: Arc<HistoryTextAdmitFn>,
    ) -> Self {
        Self {
            kind,
            limit,
            after,
            admit,
        }
    }
}

impl Collector for RelevanceCollector {
    type Fruit = RelevanceFruit;
    type Child = RelevanceSegmentCollector;

    fn for_segment(
        &self,
        _segment_local_id: SegmentOrdinal,
        segment: &SegmentReader,
    ) -> tantivy::Result<Self::Child> {
        let fast = segment.fast_fields();
        let committer_time_ms = fast.u64(KindSchema::committer_time_column())?;
        let sha = fast.bytes(KindSchema::sha_column())?.ok_or_else(|| {
            tantivy::TantivyError::SchemaError("sha column is missing".to_string())
        })?;
        let path = match self.kind {
            HistoryTextKindV1::Commit => None,
            HistoryTextKindV1::Diff => {
                Some(fast.str(KindSchema::path_column())?.ok_or_else(|| {
                    tantivy::TantivyError::SchemaError("path column is missing".to_string())
                })?)
            }
        };
        Ok(RelevanceSegmentCollector {
            columns: RowColumns {
                committer_time_ms,
                sha,
                path,
                sha_buffer: Vec::with_capacity(CommitSha::ZERO.as_bytes().len()),
                path_buffer: String::new(),
            },
            limit: self.limit,
            after: self.after.clone(),
            admit: Arc::clone(&self.admit),
            kept: BinaryHeap::new(),
            examined: 0,
            matched: 0,
            error: None,
        })
    }

    fn requires_scoring(&self) -> bool {
        true
    }

    fn merge_fruits(&self, segment_fruits: Vec<RelevanceFruit>) -> tantivy::Result<RelevanceFruit> {
        let mut merged = RelevanceFruit {
            examined: 0,
            matched: 0,
            hits: Vec::new(),
            error: None,
        };
        for fruit in segment_fruits {
            merged.examined = merged.examined.saturating_add(fruit.examined);
            merged.matched = merged.matched.saturating_add(fruit.matched);
            if merged.error.is_none() {
                merged.error = fruit.error;
            }
            merged.hits.extend(fruit.hits);
        }
        merged.hits.sort();
        merged.hits.truncate(self.limit);
        Ok(merged)
    }
}

/// The fast columns of one segment that identify and order a document.
struct RowColumns {
    committer_time_ms: Column<u64>,
    sha: BytesColumn,
    path: Option<StrColumn>,
    sha_buffer: Vec<u8>,
    path_buffer: String,
}

impl RowColumns {
    fn hit(&mut self, doc: DocId, score: HistoryScoreV1) -> Result<HistoryTextHitV1, CoreError> {
        let committer_time_ms = self.committer_time_ms.first(doc).ok_or_else(|| {
            CoreError::Storage(format!(
                "history text index: document {doc} has no committer time column value"
            ))
        })?;
        let sha_ord = self.sha.term_ords(doc).next().ok_or_else(|| {
            CoreError::Storage(format!(
                "history text index: document {doc} has no sha column value"
            ))
        })?;
        self.sha_buffer.clear();
        let found = self
            .sha
            .ord_to_bytes(sha_ord, &mut self.sha_buffer)
            .map_err(|err| {
                CoreError::Storage(format!(
                    "history text index: resolve sha of document {doc}: {err}"
                ))
            })?;
        if !found {
            return Err(CoreError::Storage(format!(
                "history text index: sha ordinal of document {doc} is not in the dictionary"
            )));
        }
        let sha_bytes: [u8; 20] = self.sha_buffer.as_slice().try_into().map_err(|_err| {
            CoreError::Storage(format!(
                "history text index: document {doc} carries a {}-byte sha",
                self.sha_buffer.len()
            ))
        })?;
        let sha = CommitSha::from_bytes(sha_bytes);
        let key = match &self.path {
            None => HistoryTextDocKeyV1::Commit { sha },
            Some(path) => {
                let path_ord = path.term_ords(doc).next().ok_or_else(|| {
                    CoreError::Storage(format!(
                        "history text index: diff document {doc} has no path column value"
                    ))
                })?;
                self.path_buffer.clear();
                let found = path
                    .ord_to_str(path_ord, &mut self.path_buffer)
                    .map_err(|err| {
                        CoreError::Storage(format!(
                            "history text index: resolve path of document {doc}: {err}"
                        ))
                    })?;
                if !found {
                    return Err(CoreError::Storage(format!(
                        "history text index: path ordinal of document {doc} is not in the dictionary"
                    )));
                }
                HistoryTextDocKeyV1::Diff {
                    sha,
                    file_path: self.path_buffer.clone(),
                }
            }
        };
        Ok(HistoryTextHitV1 {
            key,
            committer_time_ms,
            score,
        })
    }
}

/// The per-segment half of [`RelevanceCollector`].
pub(super) struct RelevanceSegmentCollector {
    columns: RowColumns,
    limit: usize,
    after: Option<HistoryTextHitV1>,
    admit: Arc<HistoryTextAdmitFn>,
    /// A max-heap under the relevance order: its top is the worst hit
    /// kept, which leaves first when the page is full.
    kept: BinaryHeap<HistoryTextHitV1>,
    examined: u64,
    matched: u64,
    error: Option<CoreError>,
}

impl RelevanceSegmentCollector {
    fn collect_checked(&mut self, doc: DocId, score: Score) -> Result<(), CoreError> {
        let score = HistoryScoreV1::try_new(score).map_err(|err| {
            CoreError::Storage(format!(
                "history text index: document {doc} scored {score}: {err}"
            ))
        })?;
        if let Some(after) = &self.after {
            // A better score than the cursor's is at or before the cursor
            // whatever the tie-break; only an equal score needs the rest of
            // the key, and a worse one is always after.
            if score > after.score {
                return Ok(());
            }
        }
        let hit = self.columns.hit(doc, score)?;
        if self
            .after
            .as_ref()
            .is_some_and(|after| hit.cmp(after) != std::cmp::Ordering::Greater)
        {
            return Ok(());
        }
        if !(self.admit)(&hit)? {
            return Ok(());
        }
        self.matched = self.matched.saturating_add(1);
        if self.limit == 0 {
            return Ok(());
        }
        if self.kept.len() < self.limit {
            self.kept.push(hit);
            return Ok(());
        }
        if self.kept.peek().is_some_and(|worst| hit < *worst) {
            let _evicted = self.kept.pop();
            self.kept.push(hit);
        }
        Ok(())
    }
}

impl SegmentCollector for RelevanceSegmentCollector {
    type Fruit = RelevanceFruit;

    fn collect(&mut self, doc: DocId, score: Score) {
        self.examined = self.examined.saturating_add(1);
        if self.error.is_some() {
            return;
        }
        if let Err(error) = self.collect_checked(doc, score) {
            self.error = Some(error);
        }
    }

    fn harvest(self) -> RelevanceFruit {
        RelevanceFruit {
            examined: self.examined,
            matched: self.matched,
            hits: self.kept.into_sorted_vec(),
            error: self.error,
        }
    }
}
