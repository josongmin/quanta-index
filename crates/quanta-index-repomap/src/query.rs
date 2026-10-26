//! The `RepoMap` query: a bounded selection over an indexed snapshot
//! (QI-BB-008).
//!
//! A query used to clone every entry of the snapshot, sort them all, and
//! return every row with a flag saying whether it made the cut. Now the
//! snapshot is shared and indexed once, a query keeps at most the entries it
//! can return, and the response carries the included rows and a count of
//! the rest. Selection starts with a `top_k` prefix; variable token costs
//! may require wider prefixes and repeated scoring passes to find rows that
//! fit. It stops widening when the scan proves no remaining row can fit.
//!
//! Ranking is unchanged: exact focus subjects first, then entries under a
//! focused owner path, then query-term matches, then the materialized
//! score, ties broken by identity. Inclusion walks that order and admits an
//! entry while `top_k` and the token budget allow; the first entry is
//! admitted over the budget so a query never returns nothing for a budget
//! alone.

pub(crate) mod ranking;

use std::collections::BTreeSet;

use quanta_index_contract::{
    RepoMapDocType, RepoMapEntryDto, RepoMapQueryRequest, RepoMapQueryResponse,
};
use quanta_index_core::{CoreError, RepoMapPolicy};

use crate::model::{RepoMapEntry, RepoMapIndexedSnapshot, RepoMapSnapshotIndex};

use self::ranking::{RankCandidate, RankPlan, RankScanner, select, tokenize};

pub struct RepoMapQueryEngine;

struct SnapshotCandidate<'a> {
    entry: &'a RepoMapEntry,
    folded_search_text: &'a str,
    position: usize,
}

impl RankCandidate for SnapshotCandidate<'_> {
    fn identity(&self) -> &str {
        &self.entry.subject_identity
    }

    fn doc_type(&self) -> RepoMapDocType {
        self.entry.subject_doc_type
    }

    fn owner_path(&self) -> &str {
        &self.entry.owner_path
    }

    fn folded_search_text(&self) -> &str {
        self.folded_search_text
    }

    fn final_score_millis(&self) -> u32 {
        self.entry.final_score_millis
    }

    fn token_budget_hint(&self) -> u32 {
        self.entry.token_budget_hint
    }

    fn source_ordinal(&self) -> usize {
        self.position
    }
}

struct SnapshotScanner<'a> {
    entries: &'a [RepoMapEntry],
    index: &'a RepoMapSnapshotIndex,
    candidates: Option<&'a [usize]>,
}

impl<'a> RankScanner for SnapshotScanner<'a> {
    type Candidate = SnapshotCandidate<'a>;
    type Error = CoreError;
    type Grant = ();

    fn resource_limited() -> Self::Error {
        CoreError::Storage("repomap rank allocation refused".to_string())
    }

    fn reserve_prefix(&mut self, _bytes: usize) -> Result<Self::Grant, Self::Error> {
        Ok(())
    }

    fn scan(
        &mut self,
        sink: &mut dyn FnMut(Self::Candidate) -> Result<(), Self::Error>,
    ) -> Result<(), Self::Error> {
        let entries = self.entries;
        let index = self.index;
        let candidates = self.candidates;
        let mut consider = |position: usize, entry: &'a RepoMapEntry| {
            sink(SnapshotCandidate {
                entry,
                folded_search_text: index
                    .folded_search_text
                    .get(position)
                    .map_or("", String::as_str),
                position,
            })
        };
        match candidates {
            Some(positions) => {
                for &position in positions {
                    if let Some(entry) = entries.get(position) {
                        consider(position, entry)?;
                    }
                }
            }
            None => {
                for (position, entry) in entries.iter().enumerate() {
                    consider(position, entry)?;
                }
            }
        }
        Ok(())
    }
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
        let top_k_limit = usize::try_from(request.top_k).map_or(usize::MAX, std::convert::identity);
        let plan = RankPlan {
            focus_keys: &focus_keys,
            focus_owner_paths: &focus_owner_paths,
            query_terms: &query_terms,
            top_k_limit,
            initial_prefix: top_k_limit.max(1).min(universe_size.max(1)),
            token_budget: request.token_budget,
        };
        let mut scanner = SnapshotScanner {
            entries,
            index,
            candidates: if focus_owner_paths.is_empty() {
                None
            } else {
                Some(&candidates)
            },
        };
        let selection = select(&mut scanner, &plan)?;
        let mut response_entries: Vec<RepoMapEntryDto> =
            Vec::with_capacity(selection.included.len());
        for (candidate, rank) in &selection.included {
            response_entries.push(candidate.entry.to_dto(*rank));
        }
        Ok(RepoMapQueryResponse {
            repo_id: snapshot.snapshot.repo_id.clone(),
            revision_id: snapshot.snapshot.revision_id.clone(),
            manifest_generation: snapshot.snapshot.manifest_generation,
            snapshot_meta: snapshot.snapshot.snapshot_meta.clone(),
            entries: response_entries,
            dropped_entries_count: selection.dropped_entries_count,
            drop_reason_codes: selection.drop_reason_codes,
            degraded_reason_codes: selection.degraded_reason_codes,
        })
    }
}
