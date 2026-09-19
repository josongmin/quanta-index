//! Which history records a query admits and what they project to.
//!
//! The one predicate both history orders admit a row through
//! ([`history_commit_matches`] / [`history_diff_matches`]: the query's
//! filters plus its text expression, under the one text normalizer), the
//! query's record kind, the cursor-kind check, and the candidates a
//! matched record becomes. [`super::history`] cuts `recency` pages over
//! them and [`super::history_relevance`] joins `relevance` hits back
//! through them, so the same query counts the same rows under either
//! order (QI-BB-023 보완 #3).

use crate::query_dispatcher::errors::{history_invalid_request, history_invalid_timeref};
use crate::query_dispatcher::text_plane::{expr_matches, leaf_matches_text, matches_text};
use crate::query_dispatcher::timeref::{parse_history_timeref_ms, unix_seconds_from_ms};
use crate::readiness::{HistoryAuthorityState, history_diff_search_text};
use quanta_index_contract::lex::CommitSha;
use quanta_index_contract::{
    CommitCandidate, DiffCandidate, HistoryCursor, HistoryScoreV1, LqFilter, LqQuery, LqType,
    QueryResultWindowV1,
};
use quanta_index_core::CoreError;

/// One page of history results, in the order it was cut under.
#[derive(Debug)]
pub(super) struct HistoryPage {
    pub(super) commits: Vec<CommitCandidate>,
    pub(super) diffs: Vec<DiffCandidate>,
    pub(super) window: QueryResultWindowV1,
    pub(super) examined: u64,
    pub(super) next_cursor: Option<HistoryCursor>,
}

pub(super) fn history_query_type(query: &LqQuery) -> Option<LqType> {
    for filter in &query.filters {
        if let LqFilter::Type { kind } = filter {
            return Some(*kind);
        }
    }
    None
}

/// A commit page continues from a commit cursor and a diff page from a
/// diff cursor, whatever the order.
pub(super) fn ensure_cursor_kind(
    kind: HistoryQueryKind,
    cursor: Option<&HistoryCursor>,
) -> Result<(), CoreError> {
    match (kind, cursor) {
        (HistoryQueryKind::Commit, Some(cursor)) if cursor.file_path.is_some() => Err(
            history_invalid_request("history: a commit page cannot continue from a diff cursor"),
        ),
        (HistoryQueryKind::Diff, Some(cursor)) if cursor.file_path.is_none() => Err(
            history_invalid_request("history: a diff page cannot continue from a commit cursor"),
        ),
        (HistoryQueryKind::Commit | HistoryQueryKind::Diff, Some(_) | None) => Ok(()),
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum HistoryQueryKind {
    Commit,
    Diff,
}

pub(super) fn resolve_history_query_kind(query: &LqQuery) -> Result<HistoryQueryKind, CoreError> {
    let has_diff_only_filters = query.filters.iter().any(|filter| {
        matches!(
            filter,
            LqFilter::File { .. }
                | LqFilter::DiffAdded { .. }
                | LqFilter::DiffRemoved { .. }
                | LqFilter::DiffTouched { .. }
        )
    });
    match history_query_type(query) {
        Some(LqType::Commit) => {
            if has_diff_only_filters {
                return Err(history_invalid_request(
                    "history: `file:` and `diff.*` filters require `type:diff`",
                ));
            }
            Ok(HistoryQueryKind::Commit)
        }
        Some(LqType::Diff) => Ok(HistoryQueryKind::Diff),
        Some(LqType::File | LqType::Path | LqType::Symbol | LqType::Repo) => Err(
            history_invalid_request("history: only `type:commit` and `type:diff` are executable"),
        ),
        None => Err(history_invalid_request(
            "history: explicit `type:commit` or `type:diff` is required",
        )),
    }
}

/// Whether a commit matches the query: its filters and its text expression.
///
/// This is the row predicate of both orders: `recency` runs it over every
/// record, `relevance` over every record the text index enumerates
/// (QI-BB-023 보완 #3).
pub(super) fn history_commit_matches(
    query: &LqQuery,
    state: &HistoryAuthorityState,
    record: &quanta_index_contract::lex::CommitRecord,
) -> Result<bool, CoreError> {
    if !history_commit_filters_match(query, state, record)? {
        return Ok(false);
    }
    expr_matches(&query.expr, &mut |leaf| {
        leaf_matches_text("history", leaf, record.message.as_ref(), &query.options)
    })
}

/// Whether a commit passes every filter of the query, the text
/// expression aside.
fn history_commit_filters_match(
    query: &LqQuery,
    state: &HistoryAuthorityState,
    record: &quanta_index_contract::lex::CommitRecord,
) -> Result<bool, CoreError> {
    for filter in &query.filters {
        match filter {
            LqFilter::Type { kind } => {
                if !matches!(kind, LqType::Commit) {
                    return Ok(false);
                }
            }
            LqFilter::File { .. } => return Ok(false),
            LqFilter::Rev { spec } => {
                if !history_rev_matches(state, spec, &record.sha) {
                    return Ok(false);
                }
            }
            LqFilter::Author { pattern } => {
                if !matches_text(pattern, record.author.as_ref(), &query.options) {
                    return Ok(false);
                }
            }
            LqFilter::Committer { pattern } => {
                if !matches_text(pattern, record.committer.as_ref(), &query.options) {
                    return Ok(false);
                }
            }
            LqFilter::Message { pattern } => {
                if !matches_text(pattern, record.message.as_ref(), &query.options) {
                    return Ok(false);
                }
            }
            LqFilter::Content { leaf } => {
                if !leaf_matches_text("history", leaf, record.message.as_ref(), &query.options)? {
                    return Ok(false);
                }
            }
            LqFilter::Before { timeref } => {
                if !history_committer_time_before(record.committer_time_ms, timeref)? {
                    return Ok(false);
                }
            }
            LqFilter::After { timeref } => {
                if !history_committer_time_after(record.committer_time_ms, timeref)? {
                    return Ok(false);
                }
            }
            LqFilter::Since { timeref } => {
                if !history_committer_time_since(state, record.committer_time_ms, timeref)? {
                    return Ok(false);
                }
            }
            LqFilter::Until { timeref } => {
                if !history_committer_time_until(record.committer_time_ms, timeref)? {
                    return Ok(false);
                }
            }
            LqFilter::DiffAdded { .. }
            | LqFilter::DiffRemoved { .. }
            | LqFilter::DiffTouched { .. } => {
                return Ok(false);
            }
            LqFilter::Repo { .. }
            | LqFilter::Lang { .. }
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
            | LqFilter::Context { .. } => {}
        }
    }
    Ok(true)
}

/// Whether a diff hunk matches the query: its filters and its text expression.
///
/// The expression runs over the hunk's search text — the text the history
/// text index scores. The row predicate of both orders, as
/// [`history_commit_matches`] is for commits.
pub(super) fn history_diff_matches(
    query: &LqQuery,
    state: &HistoryAuthorityState,
    key: &crate::readiness::HistoryDiffKey,
    record: &quanta_index_contract::lex::DiffHunkRecord,
    commit: &quanta_index_contract::lex::CommitRecord,
) -> Result<bool, CoreError> {
    if !history_diff_filters_match(query, state, key, record, commit)? {
        return Ok(false);
    }
    let diff_text = history_diff_search_text(key, record);
    expr_matches(&query.expr, &mut |leaf| {
        leaf_matches_text("history", leaf, &diff_text, &query.options)
    })
}

/// Whether a diff hunk passes every filter of the query, the text
/// expression aside.
fn history_diff_filters_match(
    query: &LqQuery,
    state: &HistoryAuthorityState,
    key: &crate::readiness::HistoryDiffKey,
    record: &quanta_index_contract::lex::DiffHunkRecord,
    commit: &quanta_index_contract::lex::CommitRecord,
) -> Result<bool, CoreError> {
    for filter in &query.filters {
        match filter {
            LqFilter::Type { kind } => {
                if !matches!(kind, LqType::Diff) {
                    return Ok(false);
                }
            }
            LqFilter::File { pattern, .. } => {
                if !matches_text(pattern, key.file_path(), &query.options) {
                    return Ok(false);
                }
            }
            LqFilter::Rev { spec } => {
                if !history_rev_matches(state, spec, &commit.sha) {
                    return Ok(false);
                }
            }
            LqFilter::Author { pattern } => {
                if !matches_text(pattern, commit.author.as_ref(), &query.options) {
                    return Ok(false);
                }
            }
            LqFilter::Committer { pattern } => {
                if !matches_text(pattern, commit.committer.as_ref(), &query.options) {
                    return Ok(false);
                }
            }
            LqFilter::Message { pattern } => {
                if !matches_text(pattern, commit.message.as_ref(), &query.options) {
                    return Ok(false);
                }
            }
            LqFilter::Content { leaf } => {
                if !leaf_matches_text(
                    "history",
                    leaf,
                    &history_diff_search_text(key, record),
                    &query.options,
                )? {
                    return Ok(false);
                }
            }
            LqFilter::Before { timeref } => {
                if !history_committer_time_before(commit.committer_time_ms, timeref)? {
                    return Ok(false);
                }
            }
            LqFilter::After { timeref } => {
                if !history_committer_time_after(commit.committer_time_ms, timeref)? {
                    return Ok(false);
                }
            }
            LqFilter::Since { timeref } => {
                if !history_committer_time_since(state, commit.committer_time_ms, timeref)? {
                    return Ok(false);
                }
            }
            LqFilter::Until { timeref } => {
                if !history_committer_time_until(commit.committer_time_ms, timeref)? {
                    return Ok(false);
                }
            }
            LqFilter::DiffAdded { pattern } => {
                if !matches_text(pattern, record.added_text.as_ref(), &query.options) {
                    return Ok(false);
                }
            }
            LqFilter::DiffRemoved { pattern } => {
                if !matches_text(pattern, record.removed_text.as_ref(), &query.options) {
                    return Ok(false);
                }
            }
            LqFilter::DiffTouched { pattern } => {
                if !matches_text(pattern, record.touched_text.as_ref(), &query.options) {
                    return Ok(false);
                }
            }
            LqFilter::Repo { .. }
            | LqFilter::Lang { .. }
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
            | LqFilter::Context { .. } => {}
        }
    }
    Ok(true)
}

fn history_rev_matches(state: &HistoryAuthorityState, spec: &str, sha: &CommitSha) -> bool {
    if sha.to_string() == spec {
        return true;
    }
    if state
        .refs()
        .get(spec)
        .is_some_and(|resolved| resolved == sha)
    {
        return true;
    }
    state
        .tags()
        .get(spec)
        .is_some_and(|resolved| resolved == sha)
}

/// The commit row of a page; `score` is its relevance score under that
/// order and absent under recency.
pub(super) fn commit_candidate_from_record(
    record: &quanta_index_contract::lex::CommitRecord,
    score: Option<HistoryScoreV1>,
) -> CommitCandidate {
    CommitCandidate {
        sha: record.sha,
        parent_ids: record.parents.clone(),
        committed_at_unix_s: unix_seconds_from_ms(record.committer_time_ms),
        author: record.author.to_string(),
        committer: record.committer.to_string(),
        message: record.message.to_string(),
        is_merge: record.is_merge,
        tags: record.tags.iter().map(ToString::to_string).collect(),
        score,
    }
}

/// The diff row of a page; `score` as for [`commit_candidate_from_record`].
pub(super) fn diff_candidate_from_record(
    key: &crate::readiness::HistoryDiffKey,
    record: &quanta_index_contract::lex::DiffHunkRecord,
    score: Option<HistoryScoreV1>,
) -> DiffCandidate {
    DiffCandidate {
        repo_relative_path: key.file_path().to_string(),
        hunk_header: record.hunk_header.to_string(),
        side: record.side,
        line_start: record.byte_start,
        line_end: record.byte_end,
        snippet: history_diff_snippet(record),
        score,
    }
}

fn history_diff_snippet(record: &quanta_index_contract::lex::DiffHunkRecord) -> String {
    if !record.touched_text.is_empty() {
        return record.touched_text.to_string();
    }
    if !record.added_text.is_empty() {
        return record.added_text.to_string();
    }
    if !record.removed_text.is_empty() {
        return record.removed_text.to_string();
    }
    record.hunk_header.to_string()
}

fn history_committer_time_before(committer_time_ms: u64, timeref: &str) -> Result<bool, CoreError> {
    let boundary_ms = parse_history_timeref_ms(timeref)?;
    Ok(committer_time_ms < boundary_ms)
}

fn history_committer_time_after(committer_time_ms: u64, timeref: &str) -> Result<bool, CoreError> {
    let boundary_ms = parse_history_timeref_ms(timeref)?;
    Ok(committer_time_ms > boundary_ms)
}

fn history_committer_time_since(
    state: &HistoryAuthorityState,
    committer_time_ms: u64,
    timeref: &str,
) -> Result<bool, CoreError> {
    let boundary_ms = resolve_history_since_timeref_ms(state, timeref)?;
    Ok(committer_time_ms >= boundary_ms)
}

fn history_committer_time_until(committer_time_ms: u64, timeref: &str) -> Result<bool, CoreError> {
    let boundary_ms = parse_history_timeref_ms(timeref)?;
    Ok(committer_time_ms <= boundary_ms)
}

pub(super) fn validate_history_since_timeref(timeref: &str) -> Result<(), CoreError> {
    if let Some(spec) = timeref.strip_prefix("commit:") {
        if spec.is_empty() {
            return Err(history_invalid_timeref(
                "history: since.commit requires a non-empty commit/ref/tag spec",
            ));
        }
        return Ok(());
    }
    let timeref = timeref.strip_prefix("time:").unwrap_or(timeref);
    let _: u64 = parse_history_timeref_ms(timeref)?;
    Ok(())
}

fn resolve_history_since_timeref_ms(
    state: &HistoryAuthorityState,
    timeref: &str,
) -> Result<u64, CoreError> {
    if let Some(spec) = timeref.strip_prefix("commit:") {
        return resolve_history_commit_timeref_ms(state, spec);
    }
    let timeref = timeref.strip_prefix("time:").unwrap_or(timeref);
    parse_history_timeref_ms(timeref)
}

fn resolve_history_commit_timeref_ms(
    state: &HistoryAuthorityState,
    spec: &str,
) -> Result<u64, CoreError> {
    if let Ok(sha) = CommitSha::from_hex(spec)
        && let Some(record) = state.commits().get(&sha)
    {
        return Ok(record.committer_time_ms);
    }
    if let Some(sha) = state.refs().get(spec)
        && let Some(record) = state.commits().get(sha)
    {
        return Ok(record.committer_time_ms);
    }
    if let Some(sha) = state.tags().get(spec)
        && let Some(record) = state.commits().get(sha)
    {
        return Ok(record.committer_time_ms);
    }
    Err(history_invalid_timeref(format!(
        "history: since.commit `{spec}` does not resolve to a materialized commit"
    )))
}
