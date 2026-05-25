use std::collections::BTreeSet;

use quanta_index_contract::{RepoMapQueryRequest, RepoMapQueryResponse};
use quanta_index_core::{CoreError, RepoMapPolicy};

use crate::model::RepoMapSnapshotV1;

pub struct RepoMapQueryEngine;

impl RepoMapQueryEngine {
    pub fn query(
        snapshot: &RepoMapSnapshotV1,
        request: &RepoMapQueryRequest,
    ) -> Result<RepoMapQueryResponse, CoreError> {
        RepoMapPolicy::validate_query(request)?;

        let query_terms = tokenize(&request.query_text);
        let focus_keys = request
            .focus_subjects
            .iter()
            .map(|focus| {
                (
                    focus.subject_identity.clone(),
                    focus.subject_doc_type.clone(),
                )
            })
            .collect::<BTreeSet<_>>();
        let mut entries = snapshot.entries.clone();
        let focus_owner_paths = entries
            .iter()
            .filter(|entry| {
                focus_keys.contains(&(
                    entry.subject_identity.clone(),
                    entry.subject_doc_type.clone(),
                ))
            })
            .map(|entry| entry.owner_path.clone())
            .collect::<BTreeSet<_>>();

        let mut degraded_reason_codes = BTreeSet::<String>::new();
        if !focus_keys.is_empty() && focus_owner_paths.is_empty() {
            let _inserted = degraded_reason_codes.insert("focus_subjects_unresolved".to_string());
        }
        if !focus_owner_paths.is_empty() {
            entries.retain(|entry| {
                focus_keys.contains(&(
                    entry.subject_identity.clone(),
                    entry.subject_doc_type.clone(),
                )) || focus_owner_paths.contains(&entry.owner_path)
            });
        }

        let any_query_match = entries
            .iter()
            .any(|entry| query_match_score(entry, &query_terms) > 0);
        if !query_terms.is_empty() && !any_query_match {
            let _inserted = degraded_reason_codes.insert("query_terms_unmatched".to_string());
        }

        entries.sort_by(|lhs, rhs| {
            exact_focus(rhs, &focus_keys)
                .cmp(&exact_focus(lhs, &focus_keys))
                .then(
                    owner_path_focus(rhs, &focus_owner_paths)
                        .cmp(&owner_path_focus(lhs, &focus_owner_paths)),
                )
                .then(
                    query_match_score(rhs, &query_terms).cmp(&query_match_score(lhs, &query_terms)),
                )
                .then(rhs.final_score_millis.cmp(&lhs.final_score_millis))
                .then(lhs.subject_identity.cmp(&rhs.subject_identity))
        });

        let top_k_limit = saturating_usize_from_u32(request.top_k);
        let mut included_count = 0_usize;
        let mut consumed_tokens = 0_u32;
        let mut drop_reason_codes = BTreeSet::<String>::new();
        let mut forced_budget_floor = false;

        for entry in &mut entries {
            let token_hint = entry.token_budget_hint.max(1);
            let within_top_k = included_count < top_k_limit;
            let within_budget = consumed_tokens.saturating_add(token_hint) <= request.token_budget;
            let apply_budget_floor = included_count == 0 && within_top_k && !within_budget;
            if within_top_k && (within_budget || apply_budget_floor) {
                if apply_budget_floor {
                    forced_budget_floor = true;
                }
                included_count = included_count.saturating_add(1);
                consumed_tokens = consumed_tokens.saturating_add(token_hint);
                entry.included = true;
                entry.rank = saturating_u32_from_usize(included_count);
                continue;
            }
            entry.included = false;
            entry.rank = 0;
            let code = if within_top_k {
                "token_budget_exhausted"
            } else {
                "top_k_exhausted"
            };
            let _inserted = drop_reason_codes.insert(code.to_string());
        }

        if forced_budget_floor {
            let _inserted = degraded_reason_codes.insert("token_budget_floor_applied".to_string());
        }

        let dropped_entries_count =
            saturating_u32_from_usize(entries.iter().filter(|entry| !entry.included).count());

        Ok(RepoMapQueryResponse {
            repo_id: snapshot.repo_id.clone(),
            revision_id: snapshot.revision_id.clone(),
            manifest_generation: snapshot.manifest_generation,
            snapshot_meta: snapshot.snapshot_meta.clone(),
            entries: entries.into_iter().map(|entry| entry.to_dto()).collect(),
            dropped_entries_count,
            drop_reason_codes: drop_reason_codes.into_iter().collect(),
            degraded_reason_codes: degraded_reason_codes.into_iter().collect(),
        })
    }
}

fn tokenize(query_text: &str) -> Vec<String> {
    query_text
        .split(|ch: char| !ch.is_ascii_alphanumeric())
        .filter(|term| !term.is_empty())
        .map(str::to_ascii_lowercase)
        .collect()
}

fn exact_focus(
    entry: &crate::model::RepoMapEntryV1,
    focus_keys: &BTreeSet<(String, String)>,
) -> bool {
    focus_keys.contains(&(
        entry.subject_identity.clone(),
        entry.subject_doc_type.clone(),
    ))
}

fn owner_path_focus(
    entry: &crate::model::RepoMapEntryV1,
    focus_owner_paths: &BTreeSet<String>,
) -> bool {
    focus_owner_paths.contains(&entry.owner_path)
}

fn query_match_score(entry: &crate::model::RepoMapEntryV1, query_terms: &[String]) -> u32 {
    if query_terms.is_empty() {
        return 0;
    }
    let haystack = entry.search_text.to_ascii_lowercase();
    query_terms
        .iter()
        .filter(|term| haystack.contains(term.as_str()))
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
