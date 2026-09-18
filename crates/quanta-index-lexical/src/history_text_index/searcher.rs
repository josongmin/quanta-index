//! An opened epoch index: the two kind indexes, served under the request
//! budget.

use std::path::Path;
use std::sync::Arc;

use quanta_index_core::{
    CoreError, HistoryTextAdmitFn, HistoryTextHitV1, HistoryTextKindV1, HistoryTextPageV1,
    HistoryTextQueryV1, HistoryTextSearcher, RequestBudgetV1,
};
use tantivy::{Index, IndexReader, ReloadPolicy};

use crate::budgeted_search::budgeted_search;
use crate::history_text_index::collector::RelevanceCollector;
use crate::history_text_index::layout::{kind_dir, tree_bytes};
use crate::history_text_index::query;
use crate::history_text_index::schema::{KindSchema, register_tokenizers};

/// One kind's opened index.
struct KindHandle {
    schema: KindSchema,
    reader: IndexReader,
}

impl KindHandle {
    fn open(epoch_dir: &Path, kind: HistoryTextKindV1) -> Result<Self, CoreError> {
        let dir = kind_dir(epoch_dir, kind);
        let index = Index::open_in_dir(&dir).map_err(|err| {
            CoreError::Storage(format!(
                "history text index: open {} index {}: {err}",
                kind.as_str(),
                dir.display()
            ))
        })?;
        register_tokenizers(&index);
        // The epoch is immutable: there is nothing to reload.
        let reader = index
            .reader_builder()
            .reload_policy(ReloadPolicy::Manual)
            .try_into()
            .map_err(|err| {
                CoreError::Storage(format!(
                    "history text index: open {} index reader: {err}",
                    kind.as_str()
                ))
            })?;
        Ok(Self {
            schema: KindSchema::build(kind),
            reader,
        })
    }
}

/// The opened index of one epoch.
pub(super) struct EpochSearcher {
    commits: KindHandle,
    diffs: KindHandle,
    resident_bytes: u64,
}

impl EpochSearcher {
    /// Open a published epoch directory the caller has proven servable.
    pub(super) fn open(epoch_dir: &Path) -> Result<Self, CoreError> {
        Ok(Self {
            commits: KindHandle::open(epoch_dir, HistoryTextKindV1::Commit)?,
            diffs: KindHandle::open(epoch_dir, HistoryTextKindV1::Diff)?,
            resident_bytes: tree_bytes(epoch_dir)?,
        })
    }

    const fn handle(&self, kind: HistoryTextKindV1) -> &KindHandle {
        match kind {
            HistoryTextKindV1::Commit => &self.commits,
            HistoryTextKindV1::Diff => &self.diffs,
        }
    }
}

impl HistoryTextSearcher for EpochSearcher {
    fn search(
        &self,
        query: &HistoryTextQueryV1,
        after: Option<&HistoryTextHitV1>,
        limit: usize,
        admit: Arc<HistoryTextAdmitFn>,
        budget: &RequestBudgetV1,
    ) -> Result<HistoryTextPageV1, CoreError> {
        if let Some(after) = after
            && after.key.kind() != query.kind
        {
            return Err(CoreError::InvalidContract(format!(
                "history text index: a {} cursor cannot continue a {} page",
                after.key.kind().as_str(),
                query.kind.as_str()
            )));
        }
        let handle = self.handle(query.kind);
        let compiled = query::compile(&handle.schema, query)?;
        let collector = RelevanceCollector::new(query.kind, limit, after.cloned(), admit);
        let searcher = handle.reader.searcher();
        let fruit = budgeted_search(
            &searcher,
            compiled.as_ref(),
            &collector,
            budget,
            "history-text:search",
        )?;
        if let Some(error) = fruit.error {
            return Err(error);
        }
        Ok(HistoryTextPageV1 {
            hits: fruit.hits,
            examined: fruit.examined,
            matched: fruit.matched,
        })
    }

    fn resident_bytes_estimate(&self) -> u64 {
        self.resident_bytes
    }
}
