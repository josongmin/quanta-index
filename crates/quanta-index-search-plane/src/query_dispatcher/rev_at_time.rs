//! `rev:at.time(...)` selection: rebind a lexical query to the reachable
//! commit at or before a timeref via the history authority.
//!
//! The history snapshot the selection walks is the one the planner's
//! selection view pinned for the requested generation (see
//! `planning.rs`); nothing here reads the ledger.

use std::collections::BTreeSet;

use quanta_index_contract::lex::CommitSha;
use quanta_index_contract::{GenerationPin, LqFilter, LqQuery, RevisionId, SearchPlaneTrackKind};
use quanta_index_core::CoreError;
use quanta_index_core::timeref::{parse_rev_at_time_spec, parse_search_timeref_ms};

use crate::ActivationCatalog;
use crate::query_dispatcher::errors::{history_invalid_timeref, history_shard_unavailable};
use crate::readiness::HistoryAuthorityState;

pub(super) struct PreparedLexicalTextQuery {
    pub(super) pin: GenerationPin,
    pub(super) query: LqQuery,
    pub(super) force_empty: bool,
}

/// The `rev:at.time(...)` selector a query carries: its timeref, parsed
/// to the boundary it names, and the explicit rev anchor beside it, if
/// any.
pub(super) struct RevAtTimeSelection {
    boundary_ms: u64,
    explicit_anchor: Option<String>,
}

/// Rebind `query` to the active lexical generation of the reachable
/// commit at or before `selection`'s timeref, walking `history` — the
/// history authority of `base_pin`'s generation — from the anchor.
///
/// A timeref before every reachable commit selects nothing: the plan is
/// then an explicit empty result at `base_pin`, never an unbound search.
pub(super) fn rebind_lexical_query_at_time(
    activation_catalog: &ActivationCatalog,
    history: &HistoryAuthorityState,
    base_pin: &GenerationPin,
    query: LqQuery,
    selection: &RevAtTimeSelection,
) -> Result<PreparedLexicalTextQuery, CoreError> {
    let explicit_anchor = selection.explicit_anchor.as_deref();
    ensure_rev_at_time_history_ready(history, explicit_anchor, base_pin)?;
    let anchor_sha = resolve_rev_at_time_anchor_sha(history, explicit_anchor, base_pin)?;
    let stripped_query = strip_rev_filters(query);
    let Some(selected_commit_sha) =
        select_reachable_commit_at_or_before(history, anchor_sha, selection.boundary_ms)?
    else {
        return Ok(PreparedLexicalTextQuery {
            pin: base_pin.clone(),
            query: stripped_query,
            force_empty: true,
        });
    };
    let rebound_revision = RevisionId::new(selected_commit_sha.to_hex());
    let rebound_pin = activation_catalog.resolve(
        &base_pin.repo_id,
        &rebound_revision,
        SearchPlaneTrackKind::Lexical,
    )?;
    Ok(PreparedLexicalTextQuery {
        pin: rebound_pin,
        query: stripped_query,
        force_empty: false,
    })
}

/// The `rev:at.time(...)` selector of `query`, if it carries one; a
/// second timeref selector, a second explicit anchor or a timeref that
/// does not parse is refused here, before anything is acquired.
pub(super) fn rev_at_time_selection(
    query: &LqQuery,
) -> Result<Option<RevAtTimeSelection>, CoreError> {
    let mut timeref: Option<&str> = None;
    let mut explicit_anchor: Option<&str> = None;
    for filter in &query.filters {
        let LqFilter::Rev { spec } = filter else {
            continue;
        };
        if let Some(payload) = parse_rev_at_time_spec(spec) {
            if timeref.replace(payload).is_some() {
                return Err(CoreError::InvalidContract(
                    "lexical: rev:at.time(...) accepts exactly one timeref selector".to_string(),
                ));
            }
            continue;
        }
        if explicit_anchor.replace(spec.as_str()).is_some() {
            return Err(CoreError::InvalidContract(
                "lexical: rev:at.time(...) accepts at most one explicit rev anchor".to_string(),
            ));
        }
    }
    let Some(timeref) = timeref else {
        return Ok(None);
    };
    let boundary_ms = parse_search_timeref_ms(timeref).ok_or_else(|| {
        history_invalid_timeref(format!(
            "history: timeref `{timeref}` is not a valid RFC3339 timestamp, named date, human phrase, or duration"
        ))
    })?;
    Ok(Some(RevAtTimeSelection {
        boundary_ms,
        explicit_anchor: explicit_anchor.map(str::to_string),
    }))
}

fn strip_rev_filters(mut query: LqQuery) -> LqQuery {
    query
        .filters
        .retain(|filter| !matches!(filter, LqFilter::Rev { .. }));
    query
}

fn ensure_rev_at_time_history_ready(
    state: &HistoryAuthorityState,
    explicit_anchor: Option<&str>,
    base_pin: &GenerationPin,
) -> Result<(), CoreError> {
    if !state.commits_materialized() {
        return Err(history_shard_unavailable(
            "history: commit shard is unavailable for rev:at.time(...) selection",
        ));
    }
    let requires_lookup_shards = explicit_anchor
        .is_some_and(|spec| CommitSha::from_hex(spec).is_err())
        || CommitSha::from_hex(base_pin.revision_id.as_str()).is_err();
    if requires_lookup_shards && !state.refs_materialized() {
        return Err(history_shard_unavailable(
            "history: ref shard is unavailable for rev:at.time(...) selection",
        ));
    }
    if requires_lookup_shards && !state.tags_materialized() {
        return Err(history_shard_unavailable(
            "history: tag shard is unavailable for rev:at.time(...) selection",
        ));
    }
    Ok(())
}

fn resolve_rev_at_time_anchor_sha(
    state: &HistoryAuthorityState,
    explicit_anchor: Option<&str>,
    base_pin: &GenerationPin,
) -> Result<CommitSha, CoreError> {
    if let Some(spec) = explicit_anchor {
        return resolve_history_anchor_sha(state, spec).ok_or_else(|| {
            CoreError::InvalidContract(format!(
                "lexical: rev:at.time(...) anchor `{spec}` does not resolve to a materialized commit"
            ))
        });
    }
    if let Some(anchor_sha) = resolve_history_anchor_sha(state, base_pin.revision_id.as_str()) {
        return Ok(anchor_sha);
    }
    if let Some(anchor_sha) = state.refs().get("HEAD") {
        return Ok(*anchor_sha);
    }
    Err(CoreError::InvalidContract(
        "lexical: rev:at.time(...) requires the selected revision to be a materialized commit/ref/tag or a materialized HEAD ref".to_string(),
    ))
}

fn resolve_history_anchor_sha(state: &HistoryAuthorityState, spec: &str) -> Option<CommitSha> {
    if let Ok(sha) = CommitSha::from_hex(spec)
        && state.commits().contains_key(&sha)
    {
        return Some(sha);
    }
    state
        .refs()
        .get(spec)
        .copied()
        .or_else(|| state.tags().get(spec).copied())
}

fn select_reachable_commit_at_or_before(
    state: &HistoryAuthorityState,
    anchor_sha: CommitSha,
    boundary_ms: u64,
) -> Result<Option<CommitSha>, CoreError> {
    let mut frontier = vec![anchor_sha];
    let mut visited = BTreeSet::new();
    let mut best: Option<(u64, CommitSha)> = None;
    while let Some(current_sha) = frontier.pop() {
        if !visited.insert(current_sha) {
            continue;
        }
        let Some(record) = state.commits().get(&current_sha) else {
            return Err(history_shard_unavailable(
                "history: rev:at.time(...) anchor traversal encountered an unmapped commit shard entry",
            ));
        };
        if record.committer_time_ms <= boundary_ms {
            match best {
                Some((best_time, best_sha))
                    if best_time > record.committer_time_ms
                        || (best_time == record.committer_time_ms && best_sha >= current_sha) => {}
                _ => best = Some((record.committer_time_ms, current_sha)),
            }
        }
        frontier.extend(record.parents.iter().copied());
    }
    Ok(best.map(|(_, sha)| sha))
}
