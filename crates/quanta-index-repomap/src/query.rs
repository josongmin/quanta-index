//! The `RepoMap` query: a bounded selection over an indexed snapshot
//! (QI-BB-008).
//!
//! A query used to clone every entry of the snapshot, sort them all, and
//! return every row with a flag saying whether it made the cut. Now the
//! snapshot is shared and indexed once, a query keeps at most the entries it
//! can return, and the response carries the included rows and a count of
//! the rest: for a fixed `top_k` the work that scales with the snapshot is
//! one scalar scoring pass, and nothing else does.
//!
//! Ranking is unchanged: exact focus subjects first, then entries under a
//! focused owner path, then query-term matches, then the materialized
//! score, ties broken by identity. Inclusion walks that order and admits an
//! entry while `top_k` and the token budget allow; the first entry is
//! admitted over the budget so a query never returns nothing for a budget
//! alone.

use std::cmp::Reverse;
use std::collections::{BTreeSet, BinaryHeap};

use quanta_index_contract::{
    RepoMapDocType, RepoMapEntryDto, RepoMapQueryRequest, RepoMapQueryResponse,
};
use quanta_index_core::{CoreError, RepoMapPolicy};

use crate::model::{RepoMapEntry, RepoMapIndexedSnapshot, RepoMapSnapshotIndex};

pub struct RepoMapQueryEngine;

/// The rank of one entry under one query; greater sorts first.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd)]
struct RankKey<'a> {
    exact_focus: bool,
    owner_path_focus: bool,
    query_match_score: u32,
    final_score_millis: u32,
    identity: Reverse<&'a str>,
}

/// The rank-ordered prefix a selection pass retained, best first.
struct RankedPrefix<'a> {
    entries: Vec<(RankKey<'a>, usize)>,
    /// Every entry the pass considered, retained or not.
    universe: usize,
    any_query_match: bool,
}

/// What one inclusion walk over a prefix decided.
struct InclusionWalk {
    /// Admitted entries as `(position, rank)`, in rank order.
    included: Vec<(usize, u32)>,
    /// How many prefix entries the walk visited before the page filled or
    /// the prefix ran out.
    walked: usize,
    budget_skips: bool,
    forced_budget_floor: bool,
}

impl RepoMapQueryEngine {
    pub fn query(
        snapshot: &RepoMapIndexedSnapshot,
        request: &RepoMapQueryRequest,
    ) -> Result<RepoMapQueryResponse, CoreError> {
        RepoMapPolicy::validate_query(request)?;
        let entries = &snapshot.snapshot.entries;
        let index = &snapshot.index;
        let query_terms = tokenize(&request.query_text);
        let focus_keys: BTreeSet<(&str, RepoMapDocType)> = request
            .focus_subjects
            .iter()
            .map(|focus| (focus.subject_identity.as_str(), focus.subject_doc_type))
            .collect();
        let focus_positions: Vec<usize> = focus_keys
            .iter()
            .filter_map(|(identity, doc_type)| {
                index
                    .by_subject
                    .get(*identity)
                    .and_then(|by_type| by_type.get(doc_type))
                    .copied()
            })
            .collect();
        let focus_owner_paths: BTreeSet<&str> = focus_positions
            .iter()
            .filter_map(|&position| entries.get(position))
            .map(|entry| entry.owner_path.as_str())
            .collect();

        // Non-empty focus_subjects resolve strictly: any unresolved focus
        // subject is a typed refusal. There is no global-fallback universe
        // behind a focus (S21-03).
        if !focus_keys.is_empty() && focus_positions.len() != focus_keys.len() {
            return Err(CoreError::Typed {
                code: quanta_index_contract::SearchPlaneErrorCodeV2::FocusSubjectNotFound,
                message: format!(
                    "repomap query: {} of {} focus subjects unresolved",
                    focus_keys.len().saturating_sub(focus_positions.len()),
                    focus_keys.len()
                ),
            });
        }
        let mut degraded_reason_codes = BTreeSet::<String>::new();
        // The candidate universe: under a focus only the focused subjects and
        // the entries sharing their owner paths; without a focus every entry.
        let candidates: Vec<usize> = if focus_owner_paths.is_empty() {
            Vec::new()
        } else {
            let mut positions: BTreeSet<usize> = focus_positions.iter().copied().collect();
            for path in &focus_owner_paths {
                if let Some(under_path) = index.by_owner_path.get(*path) {
                    positions.extend(under_path.iter().copied());
                }
            }
            positions.into_iter().collect()
        };
        let universe_size = if focus_owner_paths.is_empty() {
            entries.len()
        } else {
            candidates.len()
        };
        let top_k_limit = saturating_usize_from_u32(request.top_k);

        // Retain the best `prefix` entries; widen the prefix only when the
        // token budget skipped entries and the page is still short, since a
        // skip before the last retained entry means a smaller entry further
        // down the order could still fit.
        let mut prefix = top_k_limit.max(1).min(universe_size.max(1));
        let (ranked, walk) = loop {
            let ranked = rank_prefix(
                entries,
                index,
                if focus_owner_paths.is_empty() {
                    None
                } else {
                    Some(&candidates)
                },
                &focus_keys,
                &focus_owner_paths,
                &query_terms,
                prefix,
            );
            let walk = walk_inclusion(entries, &ranked.entries, top_k_limit, request.token_budget);
            let exhausted_prefix = ranked.entries.len() >= universe_size;
            if walk.included.len() >= top_k_limit || !walk.budget_skips || exhausted_prefix {
                break (ranked, walk);
            }
            prefix = prefix.saturating_mul(2).min(universe_size);
        };

        if !query_terms.is_empty() && !ranked.any_query_match {
            let _inserted = degraded_reason_codes.insert("query_terms_unmatched".to_string());
        }
        if walk.forced_budget_floor {
            let _inserted = degraded_reason_codes.insert("token_budget_floor_applied".to_string());
        }
        // Drops are of two kinds: an entry the walk saw while the page had
        // room but the budget did not, and every entry past the point the
        // page filled, walked or not.
        let mut drop_reason_codes = BTreeSet::<String>::new();
        if walk.budget_skips {
            let _inserted = drop_reason_codes.insert("token_budget_exhausted".to_string());
        }
        let included_count = walk.included.len();
        let page_filled = included_count >= top_k_limit;
        let entries_past_the_page = ranked.universe.saturating_sub(walk.walked);
        if page_filled && entries_past_the_page > 0 {
            let _inserted = drop_reason_codes.insert("top_k_exhausted".to_string());
        }
        let dropped_entries_count =
            saturating_u32_from_usize(ranked.universe.saturating_sub(included_count));

        let mut response_entries: Vec<RepoMapEntryDto> = Vec::with_capacity(included_count);
        for (position, rank) in walk.included {
            let Some(entry) = entries.get(position) else {
                return Err(CoreError::Storage(format!(
                    "repomap query: ranked position {position} is outside the snapshot"
                )));
            };
            response_entries.push(entry.to_dto(rank));
        }
        Ok(RepoMapQueryResponse {
            repo_id: snapshot.snapshot.repo_id.clone(),
            revision_id: snapshot.snapshot.revision_id.clone(),
            manifest_generation: snapshot.snapshot.manifest_generation,
            snapshot_meta: snapshot.snapshot.snapshot_meta.clone(),
            entries: response_entries,
            dropped_entries_count,
            drop_reason_codes: drop_reason_codes.into_iter().collect(),
            degraded_reason_codes: degraded_reason_codes.into_iter().collect(),
        })
    }
}

/// Score every candidate once and keep the best `prefix`, in rank order.
///
/// A bounded min-heap holds the retained set, so the pass allocates for
/// `prefix` entries and scans the rest without allocating.
fn rank_prefix<'a>(
    entries: &'a [RepoMapEntry],
    index: &RepoMapSnapshotIndex,
    candidates: Option<&[usize]>,
    focus_keys: &BTreeSet<(&str, RepoMapDocType)>,
    focus_owner_paths: &BTreeSet<&str>,
    query_terms: &[String],
    prefix: usize,
) -> RankedPrefix<'a> {
    let mut retained: BinaryHeap<Reverse<(RankKey<'a>, usize)>> =
        BinaryHeap::with_capacity(prefix.saturating_add(1));
    let mut any_query_match = false;
    let mut universe = 0_usize;
    let mut consider = |position: usize, entry: &'a RepoMapEntry| {
        universe = universe.saturating_add(1);
        let query_match_score = index
            .folded_search_text
            .get(position)
            .map_or(0, |folded| query_match_score(folded, query_terms));
        if query_match_score > 0 {
            any_query_match = true;
        }
        let key = RankKey {
            exact_focus: focus_keys
                .contains(&(entry.subject_identity.as_str(), entry.subject_doc_type)),
            owner_path_focus: focus_owner_paths.contains(entry.owner_path.as_str()),
            query_match_score,
            final_score_millis: entry.final_score_millis,
            identity: Reverse(entry.subject_identity.as_str()),
        };
        if retained.len() < prefix {
            retained.push(Reverse((key, position)));
        } else if retained
            .peek()
            .is_some_and(|Reverse((weakest, _))| key > *weakest)
        {
            let _evicted = retained.pop();
            retained.push(Reverse((key, position)));
        }
    };
    match candidates {
        Some(positions) => {
            for &position in positions {
                if let Some(entry) = entries.get(position) {
                    consider(position, entry);
                }
            }
        }
        None => {
            for (position, entry) in entries.iter().enumerate() {
                consider(position, entry);
            }
        }
    }
    let mut ordered: Vec<(RankKey<'a>, usize)> =
        retained.into_iter().map(|Reverse(item)| item).collect();
    ordered.sort_by(|left, right| right.0.cmp(&left.0));
    RankedPrefix {
        entries: ordered,
        universe,
        any_query_match,
    }
}

/// Walk a rank-ordered prefix and admit entries while `top_k` and the token
/// budget allow.
fn walk_inclusion(
    entries: &[RepoMapEntry],
    ordered: &[(RankKey<'_>, usize)],
    top_k_limit: usize,
    token_budget: u32,
) -> InclusionWalk {
    let mut included = Vec::with_capacity(top_k_limit.min(ordered.len()));
    let mut consumed_tokens = 0_u32;
    let mut budget_skips = false;
    let mut forced_budget_floor = false;
    let mut walked = 0_usize;
    for &(_, position) in ordered {
        if included.len() >= top_k_limit {
            break;
        }
        walked = walked.saturating_add(1);
        let token_hint = entries
            .get(position)
            .map_or(1, |entry| entry.token_budget_hint.max(1));
        let within_budget = consumed_tokens.saturating_add(token_hint) <= token_budget;
        let apply_budget_floor = included.is_empty() && !within_budget;
        if within_budget || apply_budget_floor {
            if apply_budget_floor {
                forced_budget_floor = true;
            }
            consumed_tokens = consumed_tokens.saturating_add(token_hint);
            let rank = saturating_u32_from_usize(included.len().saturating_add(1));
            included.push((position, rank));
        } else {
            budget_skips = true;
        }
    }
    InclusionWalk {
        included,
        walked,
        budget_skips,
        forced_budget_floor,
    }
}

/// Tokenize with the one shared Unicode tokenizer (NFC + full Unicode fold).
/// CJK runs stay one token; there is no route-local ASCII tokenizer here.
fn tokenize(query_text: &str) -> Vec<String> {
    quanta_index_lq_text_normalizer::tokenize(
        query_text,
        quanta_index_lq_text_normalizer::CaseMode::Folded,
    )
    .indexable()
    .map(|token| token.text.clone())
    .collect()
}

/// How many query terms the entry's folded search text contains.
fn query_match_score(folded_search_text: &str, query_terms: &[String]) -> u32 {
    if query_terms.is_empty() {
        return 0;
    }
    query_terms
        .iter()
        .filter(|term| folded_search_text.contains(term.as_str()))
        .count()
        .try_into()
        .map_or(u32::MAX, |count| count)
}

fn saturating_usize_from_u32(value: u32) -> usize {
    usize::try_from(value).map_or(usize::MAX, std::convert::identity)
}

fn saturating_u32_from_usize(value: usize) -> u32 {
    u32::try_from(value).map_or(u32::MAX, std::convert::identity)
}
