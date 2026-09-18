//! The history route's `relevance` order (QI-BB-023 follow-up #1).
//!
//! The text expression is scored by the epoch's history text index —
//! the same epoch the rows are read from — and the whole query (its
//! filters *and* its text expression, through the same predicate the
//! recency order runs) is evaluated against the row a hit names, inside
//! the index's collect, so the page stays bounded to `top_k`, the match
//! count is exact, and the rows are exactly the rows recency would count
//! (보완 #3): the index enumerates a sound superset of the expression's
//! rows with their scores, the predicate decides membership. The hits come
//! back under the relevance total order (score descending, committer time
//! descending, sha ascending, path ascending), strictly after the cursor
//! when there is one, and are joined back to the snapshot's records for
//! the response rows. The cursor is the last row's key under that order,
//! in the epoch the page was cut from.

use std::sync::Arc;

use quanta_index_contract::{
    CandidateCountV1, HistoryCursor, HistoryCursorOrderV1, HistoryOrderV1, LqQuery,
    QueryResultWindowV1,
};
use quanta_index_core::{
    CoreError, HistoryTextAdmitFn, HistoryTextDocKeyV1, HistoryTextHitV1, HistoryTextKindV1,
    HistoryTextQueryV1, HistoryTextSearcher, RequestBudgetV1,
};

use crate::query_dispatcher::errors::history_cursor_order_mismatch;
use crate::query_dispatcher::routes::history::{
    HistoryPage, HistoryQueryKind, commit_candidate_from_record, diff_candidate_from_record,
    ensure_cursor_kind, history_commit_matches, history_diff_matches, resolve_history_query_kind,
};
use crate::query_dispatcher::window::top_k_limit;
use crate::readiness::{AuxRead, HistoryAuthorityState, HistoryDiffKey};

/// The index hit a relevance cursor names.
fn hit_from_cursor(cursor: &HistoryCursor) -> Result<HistoryTextHitV1, CoreError> {
    let score = match cursor.order {
        HistoryCursorOrderV1::Relevance { score } => score,
        HistoryCursorOrderV1::Recency => {
            return Err(history_cursor_order_mismatch(
                HistoryOrderV1::Recency,
                HistoryOrderV1::Relevance,
            ));
        }
    };
    let key =
        cursor
            .file_path
            .as_ref()
            .map_or(HistoryTextDocKeyV1::Commit { sha: cursor.sha }, |path| {
                HistoryTextDocKeyV1::Diff {
                    sha: cursor.sha,
                    file_path: path.clone(),
                }
            });
    Ok(HistoryTextHitV1 {
        key,
        committer_time_ms: cursor.committer_time_ms,
        score,
    })
}

/// The cursor for `hit`, in the epoch the page was cut from.
fn cursor_from_hit(
    hit: &HistoryTextHitV1,
    aux_epoch: quanta_index_contract::AuxEpochV1,
) -> HistoryCursor {
    HistoryCursor {
        order: HistoryCursorOrderV1::Relevance { score: hit.score },
        committer_time_ms: hit.committer_time_ms,
        sha: hit.key.sha(),
        file_path: hit.key.file_path().map(str::to_string),
        aux_epoch,
    }
}

fn row_vanished(hit: &HistoryTextHitV1) -> CoreError {
    CoreError::Storage(format!(
        "history: the text index names {} {}{} which is absent from the epoch's rows",
        hit.key.kind().as_str(),
        hit.key.sha(),
        hit.key
            .file_path()
            .map(|path| format!(":{path}"))
            .unwrap_or_default()
    ))
}

/// The predicate the index runs on every visited row: the whole query —
/// filters and text expression, the recency order's own predicate —
/// against the row the hit names in the snapshot.
fn admit_predicate(
    kind: HistoryQueryKind,
    query: &LqQuery,
    state: &Arc<HistoryAuthorityState>,
) -> Arc<HistoryTextAdmitFn> {
    let query = query.clone();
    let state = Arc::clone(state);
    match kind {
        HistoryQueryKind::Commit => Arc::new(move |hit| {
            let record = state
                .commits()
                .get(&hit.key.sha())
                .ok_or_else(|| row_vanished(hit))?;
            history_commit_matches(&query, &state, record)
        }),
        HistoryQueryKind::Diff => Arc::new(move |hit| {
            let path = hit.key.file_path().ok_or_else(|| row_vanished(hit))?;
            let key = HistoryDiffKey::new(hit.key.sha(), path);
            let record = state
                .diff_hunks()
                .get(&key)
                .ok_or_else(|| row_vanished(hit))?;
            let commit = state
                .commits()
                .get(&hit.key.sha())
                .ok_or_else(|| row_vanished(hit))?;
            history_diff_matches(&query, &state, &key, record, commit)
        }),
    }
}

/// Score `query`'s expression on the epoch's index, keep the `top_k` best
/// admitted hits after `cursor`, and return them as rows in relevance
/// order with an exact match count.
pub(super) fn execute_history_relevance(
    query: &LqQuery,
    read: &AuxRead<HistoryAuthorityState>,
    searcher: &dyn HistoryTextSearcher,
    top_k: u32,
    cursor: Option<&HistoryCursor>,
    budget: &RequestBudgetV1,
) -> Result<HistoryPage, CoreError> {
    let kind = resolve_history_query_kind(query)?;
    ensure_cursor_kind(kind, cursor)?;
    let after = cursor.map(hit_from_cursor).transpose()?;
    let text_kind = match kind {
        HistoryQueryKind::Commit => HistoryTextKindV1::Commit,
        HistoryQueryKind::Diff => HistoryTextKindV1::Diff,
    };
    let page = searcher.search(
        &HistoryTextQueryV1 {
            kind: text_kind,
            expr: query.expr.clone(),
            options: query.options.clone(),
        },
        after.as_ref(),
        top_k_limit(top_k),
        admit_predicate(kind, query, &read.state),
        budget,
    )?;
    let returned = u32::try_from(page.hits.len()).map_err(|error| {
        CoreError::Storage(format!("history: page row count overflows u32: {error}"))
    })?;
    let has_more = page.matched > u64::from(returned);
    let window =
        QueryResultWindowV1::new(returned, CandidateCountV1::Exact(page.matched), has_more)
            .map_err(|error| CoreError::Storage(format!("history: result window: {error}")))?;
    let next_cursor = if has_more {
        page.hits.last().map(|hit| cursor_from_hit(hit, read.epoch))
    } else {
        None
    };
    let state = &read.state;
    let (commits, diffs) = match kind {
        HistoryQueryKind::Commit => (
            page.hits
                .iter()
                .map(|hit| {
                    state
                        .commits()
                        .get(&hit.key.sha())
                        .map(|record| commit_candidate_from_record(record, Some(hit.score)))
                        .ok_or_else(|| row_vanished(hit))
                })
                .collect::<Result<Vec<_>, CoreError>>()?,
            Vec::new(),
        ),
        HistoryQueryKind::Diff => (
            Vec::new(),
            page.hits
                .iter()
                .map(|hit| {
                    let path = hit.key.file_path().ok_or_else(|| row_vanished(hit))?;
                    let key = HistoryDiffKey::new(hit.key.sha(), path);
                    state
                        .diff_hunks()
                        .get(&key)
                        .map(|record| diff_candidate_from_record(&key, record, Some(hit.score)))
                        .ok_or_else(|| row_vanished(hit))
                })
                .collect::<Result<Vec<_>, CoreError>>()?,
        ),
    };
    Ok(HistoryPage {
        commits,
        diffs,
        window,
        examined: page.examined,
        next_cursor,
    })
}
