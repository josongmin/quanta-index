//! History query route: commit/diff matching and keyset paging over the
//! history authority, in the order the request asks for.
//!
//! A page is cut from one epoch-named snapshot of the history authority
//! (QI-BB-020 W2): a fresh walk reads the current epoch, a continuation
//! reads the epoch its cursor names, and the response says which. A
//! cursor whose epoch is no longer retained is refused typed; it is never
//! served from a newer snapshot where a row could repeat or go missing.
//!
//! Under `recency` (QI-BB-023) every record of the snapshot is evaluated
//! here — the text expression is a filter — and the newest matches are
//! kept. Under `relevance` (follow-up #1) the expression is scored by the
//! epoch's text index and the rows are joined back from the same snapshot;
//! see [`super::history_relevance`]. Both orders admit a row through one
//! predicate ([`history_commit_matches`] / [`history_diff_matches`]): the
//! query's filters plus its text expression, evaluated with the one text
//! normalizer, so the same query counts the same rows under either order
//! (보완 #3). A cursor continues only the order it was issued under.
//!
//! The snapshot and, under `relevance`, the epoch's text index are the
//! history domain of the request's read view (`read_view.rs`), acquired
//! once after the plan; the route executes against that view.

use quanta_index_contract::lex::CommitSha;
use quanta_index_contract::{
    AuxEpochV1, CursorAuxEpochKindV2, CursorAuxEpochV2, CursorRouteV2, HistoryCursor,
    HistoryCursorOrderV1, HistoryOrderV1, HistoryQueryRequest, LqFilter, LqQuery,
    QueryResultWindowV1, SearchPlaneHistoryQueryResponse, SearchPlaneTrackKind,
};
use quanta_index_core::{CoreError, QueryRouteV1, RequestBudgetV1, validate_query_top_k};

use crate::lower_lexical_text_query;
use crate::query_dispatcher::continuation::{CursorRequestContextV2, require_token_pin};
use crate::query_dispatcher::dispatcher::SearchPlaneDispatcher;
use crate::query_dispatcher::errors::{history_cursor_order_mismatch, history_shard_unavailable};
use crate::query_dispatcher::read_view::{AuxEpochPinsV1, ReadViewRequestV1};
use crate::query_dispatcher::routes::history_records::{
    HistoryPage, HistoryQueryKind, commit_candidate_from_record, diff_candidate_from_record,
    ensure_cursor_kind, history_commit_matches, history_diff_matches, resolve_history_query_kind,
    validate_history_since_timeref,
};
use crate::query_dispatcher::routes::history_relevance::execute_history_relevance;
use crate::query_dispatcher::selection::resolve_optional_selection;
use crate::query_dispatcher::text_plane::{
    ExecutableTextPlanePolicy, validate_executable_text_query,
};
use crate::query_dispatcher::timeref::parse_history_timeref_ms;
use crate::query_dispatcher::window::{pageable_window_v2, top_k_limit};
use crate::readiness::HistoryAuthorityState;

impl SearchPlaneDispatcher {
    pub(crate) fn history(
        &self,
        request: &HistoryQueryRequest,
        budget: &RequestBudgetV1,
    ) -> Result<SearchPlaneHistoryQueryResponse, CoreError> {
        budget.checkpoint("history:entry")?;
        let _accepted_top_k = validate_query_top_k(request.text_query.top_k)?;
        let lowered = lower_lexical_text_query(&request.text_query)?;
        validate_history_query(&lowered)?;
        let opened = request
            .cursor
            .as_ref()
            .map(|token| self.cursors()?.open::<HistoryCursor>(token))
            .transpose()?;
        ensure_cursor_continues_order(
            request.order,
            opened.as_ref().map(|cursor| &cursor.boundary),
        )?;
        let pin = if let Some(opened) = &opened {
            require_token_pin(
                request.text_query.generation.as_ref(),
                request.text_query.generation_selector.as_ref(),
                &opened.binding().pin,
            )?;
            opened.binding().pin.clone()
        } else {
            resolve_optional_selection(
                self.activation_catalog.as_ref(),
                request.text_query.generation.clone(),
                request.text_query.generation_selector.as_ref(),
                SearchPlaneTrackKind::Lexical,
                "history",
            )?
            .ok_or_else(|| {
                CoreError::InvalidContract("history: generation selector required".to_string())
            })?
        };
        let cursor_context = CursorRequestContextV2 {
            route: CursorRouteV2::History,
            pin: &pin,
            query: &lowered,
            constraints: &request.text_query.constraints,
            order: request.order.as_code_str(),
            cap: request.text_query.top_k,
        };
        if let Some(opened) = &opened {
            self.cursors()?.require_context(
                opened,
                &cursor_context,
                vec![CursorAuxEpochV2 {
                    kind: CursorAuxEpochKindV2::History,
                    epoch: opened.boundary.aux_epoch.get(),
                }],
            )?;
        }
        let boundary = opened.as_ref().map(|cursor| &cursor.boundary);
        let view = self.acquire_read_view(
            &ReadViewRequestV1::declare("history", QueryRouteV1::History, Some(&lowered), &pin)
                .with_epochs(AuxEpochPinsV1 {
                    history: boundary.map(|cursor| cursor.aux_epoch),
                    runtime: None,
                    structural: None,
                })
                .with_history_text(request.order == HistoryOrderV1::Relevance),
            budget,
        )?;
        let read = view.history()?;
        ensure_history_shards_ready(&read.state, &lowered)?;
        budget.checkpoint("history:execute")?;
        let page = match request.order {
            HistoryOrderV1::Recency => execute_history_query(
                &lowered,
                &read.state,
                read.epoch,
                request.text_query.top_k,
                boundary,
            )?,
            HistoryOrderV1::Relevance => execute_history_relevance(
                &lowered,
                read,
                view.history_text()?.as_ref(),
                request.text_query.top_k,
                boundary,
                budget,
            )?,
        };
        let window = pageable_window_v2(page.window, "history")?;
        let next_cursor = page
            .next_cursor
            .as_ref()
            .map(|cursor| {
                self.cursors()?.mint(
                    cursor,
                    &cursor_context,
                    vec![CursorAuxEpochV2 {
                        kind: CursorAuxEpochKindV2::History,
                        epoch: read.epoch.get(),
                    }],
                )
            })
            .transpose()?;
        Ok(SearchPlaneHistoryQueryResponse {
            generation: pin,
            order: request.order,
            commits: page.commits,
            diffs: page.diffs,
            window,
            read_epoch: read.epoch,
            examined: page.examined,
            next_cursor,
        })
    }
}

/// A cursor continues exactly the walk it was issued under.
fn ensure_cursor_continues_order(
    order: HistoryOrderV1,
    cursor: Option<&HistoryCursor>,
) -> Result<(), CoreError> {
    match cursor {
        Some(cursor) if cursor.order.order() != order => {
            Err(history_cursor_order_mismatch(cursor.order.order(), order))
        }
        Some(_) | None => Ok(()),
    }
}

pub(crate) fn validate_history_query(query: &LqQuery) -> Result<(), CoreError> {
    validate_executable_text_query(query, ExecutableTextPlanePolicy::History)?;
    validate_history_timeref_filters(query)?;
    let _: HistoryQueryKind = resolve_history_query_kind(query)?;
    Ok(())
}

fn validate_history_timeref_filters(query: &LqQuery) -> Result<(), CoreError> {
    for filter in &query.filters {
        match filter {
            LqFilter::Since { timeref } => validate_history_since_timeref(timeref)?,
            LqFilter::Before { timeref }
            | LqFilter::After { timeref }
            | LqFilter::Until { timeref } => {
                let _: u64 = parse_history_timeref_ms(timeref)?;
            }
            LqFilter::Repo { .. }
            | LqFilter::File { .. }
            | LqFilter::Lang { .. }
            | LqFilter::Rev { .. }
            | LqFilter::Author { .. }
            | LqFilter::Committer { .. }
            | LqFilter::Message { .. }
            | LqFilter::Type { .. }
            | LqFilter::Select { .. }
            | LqFilter::Dirty { .. }
            | LqFilter::Changed { .. }
            | LqFilter::Stale { .. }
            | LqFilter::Snapshot { .. }
            | LqFilter::MetaOwner { .. }
            | LqFilter::MetaService { .. }
            | LqFilter::MetaLayer { .. }
            | LqFilter::MetaSurface { .. }
            | LqFilter::Affected { .. }
            | LqFilter::InvalidatedBy { .. }
            | LqFilter::Fork { .. }
            | LqFilter::Archived { .. }
            | LqFilter::Visibility { .. }
            | LqFilter::Context { .. }
            | LqFilter::Content { .. }
            | LqFilter::DiffAdded { .. }
            | LqFilter::DiffRemoved { .. }
            | LqFilter::DiffTouched { .. } => {}
        }
    }
    Ok(())
}

#[expect(
    clippy::struct_excessive_bools,
    reason = "history shard readiness is modeled as four independent materialization bits"
)]
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
struct HistoryShardRequirements {
    commits: bool,
    refs: bool,
    tags: bool,
    diff_hunks: bool,
}

/// Every shard `query` reads must be materialized in the pinned snapshot;
/// a missing one is refused typed before the scan.
pub(super) fn ensure_history_shards_ready(
    state: &HistoryAuthorityState,
    query: &LqQuery,
) -> Result<(), CoreError> {
    let requirements = history_shard_requirements(query)?;
    if requirements.commits && !state.commits_materialized() {
        return Err(history_shard_unavailable(
            "history: commit shard is unavailable for the requested query",
        ));
    }
    if requirements.refs && !state.refs_materialized() {
        return Err(history_shard_unavailable(
            "history: ref shard is unavailable for the requested query",
        ));
    }
    if requirements.tags && !state.tags_materialized() {
        return Err(history_shard_unavailable(
            "history: tag shard is unavailable for the requested query",
        ));
    }
    if requirements.diff_hunks && !state.diff_hunks_materialized() {
        return Err(history_shard_unavailable(
            "history: diff shard is unavailable for the requested query",
        ));
    }
    Ok(())
}

fn history_shard_requirements(query: &LqQuery) -> Result<HistoryShardRequirements, CoreError> {
    let mut requirements = match resolve_history_query_kind(query)? {
        HistoryQueryKind::Commit => HistoryShardRequirements {
            commits: true,
            ..HistoryShardRequirements::default()
        },
        HistoryQueryKind::Diff => HistoryShardRequirements {
            commits: true,
            diff_hunks: true,
            ..HistoryShardRequirements::default()
        },
    };
    for filter in &query.filters {
        if let LqFilter::Rev { spec } = filter
            && CommitSha::from_hex(spec).is_err()
        {
            requirements.refs = true;
            requirements.tags = true;
        }
    }
    Ok(requirements)
}

/// One history element's position under the recency order (QI-BB-023).
///
/// Newest committer time first, then sha, then — for diffs — path. The
/// derived `Ord` is the wire contract's order, so a cursor is a rank and
/// "after the cursor" is `>`.
#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
struct HistoryRank {
    newest_first: std::cmp::Reverse<u64>,
    sha: CommitSha,
    file_path: Option<Box<str>>,
}

impl HistoryRank {
    fn for_commit(record: &quanta_index_contract::lex::CommitRecord) -> Self {
        Self {
            newest_first: std::cmp::Reverse(record.committer_time_ms),
            sha: record.sha,
            file_path: None,
        }
    }

    fn for_diff(
        commit: &quanta_index_contract::lex::CommitRecord,
        key: &crate::readiness::HistoryDiffKey,
    ) -> Self {
        Self {
            newest_first: std::cmp::Reverse(commit.committer_time_ms),
            sha: commit.sha,
            file_path: Some(key.file_path().into()),
        }
    }

    /// The rank a recency cursor names; a cursor of another order does
    /// not position a recency walk.
    fn from_cursor(cursor: &HistoryCursor) -> Result<Self, CoreError> {
        match cursor.order {
            HistoryCursorOrderV1::Recency => Ok(Self {
                newest_first: std::cmp::Reverse(cursor.committer_time_ms),
                sha: cursor.sha,
                file_path: cursor.file_path.as_deref().map(Into::into),
            }),
            HistoryCursorOrderV1::Relevance { .. } => Err(history_cursor_order_mismatch(
                HistoryOrderV1::Relevance,
                HistoryOrderV1::Recency,
            )),
        }
    }

    /// The cursor for this rank, in the epoch the page was cut from.
    fn into_cursor(self, aux_epoch: AuxEpochV1) -> HistoryCursor {
        HistoryCursor {
            order: HistoryCursorOrderV1::Recency,
            committer_time_ms: self.newest_first.0,
            sha: self.sha,
            file_path: self.file_path.map(Into::into),
            aux_epoch,
        }
    }
}

/// Keeps the `limit` smallest ranks (the newest elements) of everything
/// pushed, in `O(log limit)` per push, and counts what it saw.
struct HistoryPageSelector {
    limit: usize,
    kept: std::collections::BinaryHeap<HistoryRank>,
    matched: u64,
    examined: u64,
    after: Option<HistoryRank>,
    /// The epoch the page is cut from; its continuation names it.
    epoch: AuxEpochV1,
}

impl HistoryPageSelector {
    fn new(
        limit: usize,
        cursor: Option<&HistoryCursor>,
        epoch: AuxEpochV1,
    ) -> Result<Self, CoreError> {
        Ok(Self {
            limit,
            kept: std::collections::BinaryHeap::new(),
            matched: 0,
            examined: 0,
            after: cursor.map(HistoryRank::from_cursor).transpose()?,
            epoch,
        })
    }

    fn examined_one(&mut self) {
        self.examined = self.examined.saturating_add(1);
    }

    /// Offer a matching element; elements at or before the cursor are
    /// already on an earlier page.
    fn offer(&mut self, rank: HistoryRank) {
        if self.after.as_ref().is_some_and(|after| rank <= *after) {
            return;
        }
        self.matched = self.matched.saturating_add(1);
        self.kept.push(rank);
        if self.kept.len() > self.limit {
            // The heap's max is the oldest kept element; it leaves.
            drop(self.kept.pop());
        }
    }

    /// The page in order, its window and its continuation.
    fn finish(
        self,
    ) -> Result<(Vec<HistoryRank>, QueryResultWindowV1, Option<HistoryCursor>), CoreError> {
        let ranks = self.kept.into_sorted_vec();
        let returned = u32::try_from(ranks.len()).map_err(|error| {
            CoreError::Storage(format!("history: page row count overflows u32: {error}"))
        })?;
        let has_more = self.matched > u64::from(returned);
        let window = QueryResultWindowV1::new(
            returned,
            quanta_index_contract::CandidateCountV1::Exact(self.matched),
            has_more,
        )
        .map_err(|error| CoreError::Storage(format!("history: result window: {error}")))?;
        let next_cursor = if has_more {
            ranks
                .last()
                .cloned()
                .map(|rank| rank.into_cursor(self.epoch))
        } else {
            None
        };
        Ok((ranks, window, next_cursor))
    }
}

/// Evaluate every record of the queried kind, keep the `top_k` newest
/// matches after `cursor`, and return them in recency order with an exact
/// match count (QI-BB-023).
///
/// `epoch` is the epoch `state` is the snapshot of; the page's
/// continuation names it.
///
/// The scan is complete on purpose: the authority is keyed by sha, so the
/// newest matches can be anywhere in it, and the count the window
/// reports is exact rather than a bound.
fn execute_history_query(
    query: &LqQuery,
    state: &HistoryAuthorityState,
    epoch: AuxEpochV1,
    top_k: u32,
    cursor: Option<&HistoryCursor>,
) -> Result<HistoryPage, CoreError> {
    let kind = resolve_history_query_kind(query)?;
    ensure_cursor_kind(kind, cursor)?;
    let limit = top_k_limit(top_k);
    let mut selector = HistoryPageSelector::new(limit, cursor, epoch)?;
    match kind {
        HistoryQueryKind::Commit => {
            for record in state.commits().values() {
                selector.examined_one();
                if history_commit_matches(query, state, record)? {
                    selector.offer(HistoryRank::for_commit(record));
                }
            }
            let examined = selector.examined;
            let (ranks, window, next_cursor) = selector.finish()?;
            let commits = ranks
                .iter()
                .map(|rank| {
                    state
                        .commits()
                        .get(&rank.sha)
                        .map(|record| commit_candidate_from_record(record, None))
                        .ok_or_else(|| {
                            CoreError::Storage(format!(
                                "history: selected commit {} vanished from the snapshot",
                                rank.sha
                            ))
                        })
                })
                .collect::<Result<Vec<_>, CoreError>>()?;
            Ok(HistoryPage {
                commits,
                diffs: Vec::new(),
                window,
                examined,
                next_cursor,
            })
        }
        HistoryQueryKind::Diff => {
            for (key, record) in state.diff_hunks() {
                selector.examined_one();
                let Some(commit) = state.commits().get(&key.commit_sha()) else {
                    continue;
                };
                if history_diff_matches(query, state, key, record, commit)? {
                    selector.offer(HistoryRank::for_diff(commit, key));
                }
            }
            let examined = selector.examined;
            let (ranks, window, next_cursor) = selector.finish()?;
            let diffs = ranks
                .iter()
                .map(|rank| {
                    let path = rank.file_path.as_deref().ok_or_else(|| {
                        CoreError::Storage("history: a diff rank carries no path".to_string())
                    })?;
                    let key = crate::readiness::HistoryDiffKey::new(rank.sha, path);
                    state
                        .diff_hunks()
                        .get(&key)
                        .map(|record| diff_candidate_from_record(&key, record, None))
                        .ok_or_else(|| {
                            CoreError::Storage(format!(
                                "history: selected diff {}:{path} vanished from the snapshot",
                                rank.sha
                            ))
                        })
                })
                .collect::<Result<Vec<_>, CoreError>>()?;
            Ok(HistoryPage {
                commits: Vec::new(),
                diffs,
                window,
                examined,
                next_cursor,
            })
        }
    }
}

/// QI-BB-023 — history pages are in recency order, exact, and keyset-paged.
#[cfg(test)]
mod history_page_tests {
    use std::collections::BTreeSet;
    use std::time::Instant;

    use quanta_index_contract::DiffHunkSide;
    use quanta_index_contract::lex::{CommitRecord, CommitSha, DiffHunkRecord};
    use quanta_index_contract::{
        AuxEpochV1, HistoryCursor, HistoryCursorOrderV1, HistoryDiffHunkUpsert, HistoryIngestBatch,
        LQ_VERSION_TAG, LqExpr, LqFilter, LqLeaf, LqOptions, LqQuery, LqSpan, LqType,
        ManifestGeneration, RepoId, RevisionId,
    };
    use quanta_index_core::CoreError;

    use super::{HistoryPage, ensure_history_shards_ready, execute_history_query};
    use crate::readiness::{AuxRead, HistoryAuthorityState, Ledger};

    type TestRes = Result<(), Box<dyn std::error::Error>>;

    fn sha(byte: u8) -> CommitSha {
        CommitSha::from_bytes([byte; 20])
    }

    fn commit(sha_byte: u8, committer_time_ms: u64, message: &str) -> CommitRecord {
        CommitRecord {
            wire_version: 1,
            sha: sha(sha_byte),
            parents: Vec::new(),
            author_time_ms: committer_time_ms,
            committer_time_ms,
            applied_at_ms: committer_time_ms,
            author: "alice".into(),
            author_name: None,
            author_email: None,
            committer: "alice".into(),
            committer_name: None,
            committer_email: None,
            message: message.into(),
            is_merge: false,
            tags: Vec::new(),
        }
    }

    fn hunk(sha_byte: u8, path: &str) -> HistoryDiffHunkUpsert {
        HistoryDiffHunkUpsert {
            commit_sha: sha(sha_byte),
            file_path: path.into(),
            record: DiffHunkRecord {
                wire_version: 1,
                hunk_header: "@@ -1 +1 @@".into(),
                side: DiffHunkSide::After,
                added_text: "fix line".into(),
                removed_text: "".into(),
                touched_text: "fix line".into(),
                byte_start: 0,
                byte_end: 8,
            },
        }
    }

    /// Five matching commits whose sha order is the reverse of their time
    /// order, plus one that does not match.
    fn state(
        commits: Vec<CommitRecord>,
        hunks: Vec<HistoryDiffHunkUpsert>,
    ) -> Result<HistoryAuthorityState, CoreError> {
        let mut ledger = Ledger::new();
        ledger.apply_history_batch(
            &HistoryIngestBatch {
                repo_id: RepoId::new("r").expect("static fixture ID satisfies canonical policy"),
                revision_id: RevisionId::new("rev")
                    .expect("static fixture ID satisfies canonical policy"),
                generation: ManifestGeneration::new(1),
                manifest_digest: None,
                batch_digest: "history-order".to_string(),
                commits,
                refs: Vec::new(),
                tags: Vec::new(),
                diff_hunks: hunks,
            },
            Instant::now(),
        )?;
        ledger
            .history_state(
                &RepoId::new("r").expect("static fixture ID satisfies canonical policy"),
                &RevisionId::new("rev").expect("static fixture ID satisfies canonical policy"),
                ManifestGeneration::new(1),
            )
            .cloned()
            .ok_or_else(|| CoreError::Storage("history state missing".to_string()))
    }

    fn reversed_state() -> Result<HistoryAuthorityState, CoreError> {
        state(
            vec![
                commit(5, 100, "fix one"),
                commit(4, 200, "fix two"),
                commit(3, 300, "fix three"),
                commit(2, 400, "fix four"),
                commit(1, 500, "fix five"),
                commit(9, 600, "unrelated"),
            ],
            Vec::new(),
        )
    }

    fn query(kind: LqType) -> LqQuery {
        LqQuery {
            lq_version: LQ_VERSION_TAG,
            expr: LqExpr::Leaf(LqLeaf::Keyword("fix".to_string())),
            filters: vec![LqFilter::Type { kind }],
            options: LqOptions::defaults(),
            directives: Vec::new(),
            source_span: LqSpan::eof(0),
        }
    }

    fn page(
        state: &HistoryAuthorityState,
        kind: LqType,
        top_k: u32,
        cursor: Option<&HistoryCursor>,
    ) -> Result<HistoryPage, CoreError> {
        execute_history_query(&query(kind), state, AuxEpochV1::new(1), top_k, cursor)
    }

    /// The history snapshot the route would pin for `pin` at
    /// `cursor_epoch` (current when `None`), with `query`'s shards proven,
    /// read straight from the ledger the way the read view reads it.
    fn resolve_history_read(
        ledger: &Ledger,
        pin: &quanta_index_contract::GenerationPin,
        query: &LqQuery,
        cursor_epoch: Option<AuxEpochV1>,
        now: Instant,
    ) -> Result<AuxRead<HistoryAuthorityState>, CoreError> {
        let read = ledger
            .history_read_at(
                &pin.repo_id,
                &pin.revision_id,
                pin.manifest_generation,
                cursor_epoch,
                now,
            )?
            .ok_or_else(|| CoreError::Storage("history state missing".to_string()))?;
        ensure_history_shards_ready(&read.state, query)?;
        Ok(read)
    }

    #[test]
    fn top_k_returns_the_newest_matches_not_the_smallest_shas() -> TestRes {
        let state = reversed_state()?;
        let page = page(&state, LqType::Commit, 2, None)?;
        let shas: Vec<CommitSha> = page.commits.iter().map(|commit| commit.sha).collect();
        if shas != vec![sha(1), sha(2)] {
            return Err(format!("top-2 must be the two newest matches, got {shas:?}").into());
        }
        if page.window.returned() != 2
            || page.window.candidate_count() != quanta_index_contract::CandidateCountV1::Exact(5)
            || !page.window.has_more()
        {
            return Err(format!("window drifted: {:?}", page.window).into());
        }
        if page.examined != 6 {
            return Err(format!("every record is examined once, saw {}", page.examined).into());
        }
        let cursor = page
            .next_cursor
            .ok_or("a page with more must carry a cursor")?;
        if cursor.sha != sha(2) || cursor.committer_time_ms != 400 || cursor.file_path.is_some() {
            return Err(format!("the cursor names the last row, got {cursor:?}").into());
        }
        Ok(())
    }

    #[test]
    fn pages_partition_the_matches_in_order_without_gaps_or_overlap() -> TestRes {
        let state = reversed_state()?;
        let mut cursor: Option<HistoryCursor> = None;
        let mut seen: Vec<CommitSha> = Vec::new();
        let mut pages = 0_u32;
        loop {
            let page = page(&state, LqType::Commit, 2, cursor.as_ref())?;
            pages = pages.saturating_add(1);
            seen.extend(page.commits.iter().map(|commit| commit.sha));
            if page.window.candidate_count()
                != quanta_index_contract::CandidateCountV1::Exact(
                    5_u64
                        .saturating_sub(u64::try_from(seen.len())?)
                        .saturating_add(u64::try_from(page.commits.len())?),
                )
            {
                return Err(format!(
                    "each page counts exactly the matches after its cursor: {:?}",
                    page.window
                )
                .into());
            }
            match page.next_cursor {
                Some(next) if page.window.has_more() => cursor = Some(next),
                None if !page.window.has_more() => break,
                other => return Err(format!("has_more and cursor disagree: {other:?}").into()),
            }
            if pages > 10 {
                return Err("pagination did not terminate".into());
            }
        }
        if seen != vec![sha(1), sha(2), sha(3), sha(4), sha(5)] {
            return Err(
                format!("pages must partition the matches in recency order, got {seen:?}").into(),
            );
        }
        if pages != 3 {
            return Err(
                format!("five matches at two per page is three pages, took {pages}").into(),
            );
        }
        let distinct: BTreeSet<CommitSha> = seen.iter().copied().collect();
        if distinct.len() != seen.len() {
            return Err("a commit appeared on two pages".into());
        }
        Ok(())
    }

    #[test]
    fn equal_times_break_ties_by_sha_ascending() -> TestRes {
        let state = state(
            vec![
                commit(7, 100, "fix c"),
                commit(3, 100, "fix a"),
                commit(5, 100, "fix b"),
                commit(1, 50, "fix older"),
            ],
            Vec::new(),
        )?;
        let page = page(&state, LqType::Commit, 10, None)?;
        let shas: Vec<CommitSha> = page.commits.iter().map(|commit| commit.sha).collect();
        if shas != vec![sha(3), sha(5), sha(7), sha(1)] {
            return Err(format!("ties break by sha ascending, older last: {shas:?}").into());
        }
        if page.window.has_more() || page.next_cursor.is_some() {
            return Err("a complete page carries no continuation".into());
        }
        Ok(())
    }

    #[test]
    fn diff_pages_order_by_commit_recency_then_path_and_refuse_a_commit_cursor() -> TestRes {
        let state = state(
            vec![commit(2, 100, "fix old"), commit(1, 200, "fix new")],
            vec![
                hunk(2, "b.rs"),
                hunk(2, "a.rs"),
                hunk(1, "z.rs"),
                hunk(1, "m.rs"),
            ],
        )?;
        let first = page(&state, LqType::Diff, 3, None)?;
        let paths: Vec<&str> = first
            .diffs
            .iter()
            .map(|diff| diff.repo_relative_path.as_str())
            .collect();
        if paths != vec!["m.rs", "z.rs", "a.rs"] {
            return Err(format!("diffs order by commit recency then path, got {paths:?}").into());
        }
        let cursor = first.next_cursor.ok_or("more diffs remain")?;
        if cursor.file_path.as_deref() != Some("a.rs") || cursor.sha != sha(2) {
            return Err(format!("the diff cursor names the last hunk, got {cursor:?}").into());
        }
        let second = page(&state, LqType::Diff, 3, Some(&cursor))?;
        let paths: Vec<&str> = second
            .diffs
            .iter()
            .map(|diff| diff.repo_relative_path.as_str())
            .collect();
        if paths != vec!["b.rs"] || second.window.has_more() {
            return Err(format!("the second page holds the rest: {paths:?}").into());
        }
        // A commit cursor cannot position a diff page, nor the reverse.
        let commit_cursor = HistoryCursor {
            order: HistoryCursorOrderV1::Recency,
            committer_time_ms: 200,
            sha: sha(1),
            file_path: None,
            aux_epoch: AuxEpochV1::new(1),
        };
        match page(&state, LqType::Diff, 3, Some(&commit_cursor)) {
            Err(CoreError::InvalidContract(_)) => {}
            other => {
                return Err(format!("a commit cursor on a diff page answered {other:?}").into());
            }
        }
        match page(&state, LqType::Commit, 3, Some(&cursor)) {
            Err(CoreError::InvalidContract(_)) => {}
            other => {
                return Err(format!("a diff cursor on a commit page answered {other:?}").into());
            }
        }
        Ok(())
    }

    /// QI-BB-020 W2 — a continuation is served from the epoch its cursor
    /// names.
    ///
    /// The pages of one walk partition exactly the row set of that epoch
    /// even when an ingest lands between them; a cursor whose epoch has
    /// been pruned is refused, never served from a newer one.
    #[test]
    fn a_continuation_reads_the_epoch_its_cursor_names() -> TestRes {
        use quanta_index_contract::GenerationPin;
        use quanta_index_core::{AUX_EPOCH_EXPIRED_CODE, AUX_EPOCH_RETAIN};

        let now = Instant::now();
        let repo = RepoId::new("r").expect("static fixture ID satisfies canonical policy");
        let rev = RevisionId::new("rev").expect("static fixture ID satisfies canonical policy");
        let generation = ManifestGeneration::new(1);
        let pin = GenerationPin::new(repo.clone(), rev.clone(), generation);
        let batch = |digest: &str, commits: Vec<CommitRecord>| HistoryIngestBatch {
            repo_id: repo.clone(),
            revision_id: rev.clone(),
            generation,
            manifest_digest: None,
            batch_digest: digest.to_string(),
            commits,
            refs: Vec::new(),
            tags: Vec::new(),
            diff_hunks: Vec::new(),
        };
        let mut ledger = Ledger::new();
        // Epoch 1: five matches, times 100..500.
        ledger.apply_history_batch(
            &batch(
                "epoch-1",
                vec![
                    commit(5, 100, "fix one"),
                    commit(4, 200, "fix two"),
                    commit(3, 300, "fix three"),
                    commit(2, 400, "fix four"),
                    commit(1, 500, "fix five"),
                ],
            ),
            now,
        )?;
        let epoch_one_rows: BTreeSet<CommitSha> = [1, 2, 3, 4, 5].map(sha).into_iter().collect();
        let query = query(LqType::Commit);

        // Page one at the current epoch.
        let first_read = resolve_history_read(&ledger, &pin, &query, None, now)?;
        if first_read.epoch != AuxEpochV1::new(1) {
            return Err(
                format!("the first mutation is epoch 1, read {:?}", first_read.epoch).into(),
            );
        }
        let first = execute_history_query(&query, &first_read.state, first_read.epoch, 2, None)?;
        let cursor = first.next_cursor.ok_or("page one continues")?;
        if cursor.aux_epoch != AuxEpochV1::new(1) {
            return Err(format!("the cursor names the epoch it was cut from: {cursor:?}").into());
        }

        // An ingest between page one and page two: a commit at time 350
        // that would sort into page two by recency.
        ledger.apply_history_batch(&batch("epoch-2", vec![commit(7, 350, "fix seven")]), now)?;

        // The continuation reads epoch 1: the new commit is absent and the
        // pages partition epoch 1's row set exactly.
        let mut seen: Vec<CommitSha> = first.commits.iter().map(|c| c.sha).collect();
        let mut next = Some(cursor.clone());
        while let Some(cursor) = next.take() {
            let read = resolve_history_read(&ledger, &pin, &query, Some(cursor.aux_epoch), now)?;
            if read.epoch != AuxEpochV1::new(1) {
                return Err(
                    format!("a continuation reads its own epoch, read {:?}", read.epoch).into(),
                );
            }
            let page = execute_history_query(&query, &read.state, read.epoch, 2, Some(&cursor))?;
            seen.extend(page.commits.iter().map(|c| c.sha));
            next = page.next_cursor;
        }
        let distinct: BTreeSet<CommitSha> = seen.iter().copied().collect();
        if distinct.len() != seen.len() {
            return Err(format!("a commit appeared on two pages: {seen:?}").into());
        }
        if distinct != epoch_one_rows {
            return Err(format!("the walk must yield epoch 1's rows exactly, got {seen:?}").into());
        }

        // A fresh walk reads the current epoch and sees the new commit.
        let fresh_read = resolve_history_read(&ledger, &pin, &query, None, now)?;
        let fresh = execute_history_query(&query, &fresh_read.state, fresh_read.epoch, 10, None)?;
        if fresh_read.epoch != AuxEpochV1::new(2) || fresh.commits.len() != 6 {
            return Err(format!(
                "a fresh walk is at epoch 2 with six rows, got {:?} / {}",
                fresh_read.epoch,
                fresh.commits.len()
            )
            .into());
        }

        // `AUX_EPOCH_RETAIN` more mutations push epoch 1 out of retention.
        for step in 0..AUX_EPOCH_RETAIN {
            let byte = u8::try_from(step.saturating_add(10))?;
            let digest = format!("epoch-{}", step.saturating_add(3));
            ledger
                .apply_history_batch(&batch(&digest, vec![commit(byte, 600, "unrelated")]), now)?;
        }
        let retained = ledger
            .history_read_at(&repo, &rev, generation, None, now)?
            .ok_or("the generation exists")?
            .retained;
        if retained > AUX_EPOCH_RETAIN {
            return Err(
                format!("retained {retained} epochs exceeds the bound {AUX_EPOCH_RETAIN}").into(),
            );
        }
        match resolve_history_read(&ledger, &pin, &query, Some(cursor.aux_epoch), now) {
            Err(CoreError::Typed { code, .. }) if code == AUX_EPOCH_EXPIRED_CODE => Ok(()),
            other => Err(format!("a pruned epoch is refused expired, got {other:?}").into()),
        }
    }
}
