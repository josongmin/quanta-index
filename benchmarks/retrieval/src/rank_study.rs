//! Optional, non-scoring CodeSearch diagnostics collected after measured queries.
//!
//! Every page and explanation is obtained through the public SDK at the capture's
//! generation. Limits and refusals retain partial evidence; they never turn a
//! truncated top-k pool into a candidate-complete ranking experiment.

use std::collections::{BTreeMap, BTreeSet};
use std::time::{Duration, Instant};

use quanta_index_contract::{
    ContinuationTokenV2, GenerationPin, LexicalCandidate, QueryConstraintSetV1, TextQueryRequest,
    TextQuerySyntax, TextRankUnit,
};
use quanta_index_retrieval_bench::query_plan::{QueryInputPolicy, QueryPlan};
use quanta_index_retrieval_bench::record::QueryPack;
use quanta_index_retrieval_bench::sdk::{QueryOutcome, RankedHit};
use quanta_index_sdk::QuantaIndex;
use serde_json::{Value, json};

#[derive(Clone, Copy)]
pub(super) struct Limits {
    pub max_files: usize,
    pub max_pages: usize,
    pub timeout: Duration,
}

#[derive(Default)]
struct PoolProgress {
    total: Option<u64>,
    ids: BTreeSet<String>,
    paths: BTreeSet<String>,
    previous: Option<LexicalCandidate>,
}

impl PoolProgress {
    fn observe(
        &mut self,
        rows: &[LexicalCandidate],
        total: u64,
        eligible: u64,
    ) -> Result<(), String> {
        if self.total.is_some_and(|old| old != total)
            || total.checked_sub(self.ids.len() as u64) != Some(eligible)
            || eligible < rows.len() as u64
        {
            return Err("page work counts contradict the complete file pool".into());
        }
        self.total = Some(total);
        for row in rows {
            if !self.ids.insert(row.candidate_id.clone())
                || !self.paths.insert(row.repo_relative_path.as_str().into())
            {
                return Err("paging repeated a file identity or path".into());
            }
            if self
                .previous
                .as_ref()
                .is_some_and(|previous| !previous.order_key().order(&row.order_key()).is_lt())
            {
                return Err("paging violated the native total order".into());
            }
            self.previous = Some(row.clone());
        }
        Ok(())
    }

    fn exhausted(&self) -> bool {
        self.total == Some(self.ids.len() as u64)
    }
}

fn trace_count(
    trace: &quanta_index_contract::SearchExplanation,
    name: &str,
) -> Result<u64, String> {
    let prefix = format!("code_search.execution.{name}=");
    let values: Vec<_> = trace
        .planner_trace
        .iter()
        .filter_map(|entry| entry.detail.strip_prefix(&prefix))
        .collect();
    if values.len() != 1 {
        return Err(format!(
            "ordinary CodeSearch count {name} is absent or duplicated"
        ));
    }
    values[0]
        .parse()
        .map_err(|_| format!("invalid count {name}"))
}

fn same_original_hit(row: &LexicalCandidate, hit: &RankedHit) -> bool {
    let Some(authority) = &hit.file_authority else {
        return false;
    };
    row.candidate_id == hit.candidate_id
        && row.repo_relative_path.as_str() == hit.path
        && f64::from(row.score) == hit.score
        && row.start_line == hit.start_line
        && row.end_line == hit.end_line
        && row.snippet == hit.snippet
        && row.source.as_ref() == Some(&authority.source)
        && row.preview.as_ref() == Some(&authority.preview)
        && row.source_repo_id == authority.source_repo_id
        && row.repo_id == authority.repo_id
        && row.revision_id == authority.revision_id
        && row.manifest_generation == authority.generation
}

fn collect_one(
    client: &QuantaIndex,
    pin: &GenerationPin,
    request: &TextQueryRequest,
    original: &QueryOutcome,
    limits: Limits,
) -> Value {
    let start = Instant::now();
    let mut pages = Vec::new();
    let mut candidates = Vec::new();
    let mut explanations = Vec::new();
    let mut progress = PoolProgress::default();
    let mut cursor: Option<ContinuationTokenV2> = None;
    let mut cursor_values = BTreeSet::new();
    let mut pool_complete = false;
    let mut stop: Option<String> = None;
    let QueryOutcome::ReturnedWindow { hits, window, .. } = original else {
        return json!({"status":"not_run", "reason":"original_query_did_not_return_window",
            "pool_complete":false,"pages":[],"explanations":[],"diagnostic_ms":0.0});
    };
    while pages.len() < limits.max_pages {
        if start.elapsed() >= limits.timeout {
            stop = Some("diagnostic_deadline".into());
            break;
        }
        // Retain the original page size: cursors bind it and the first page must
        // reproduce the measured window, not a separately broadened request.
        let builder = client
            .lexical()
            .query()
            .code_search(&request.query_text)
            .pinned(pin.clone())
            .top_k(request.top_k);
        let builder = match cursor.take() {
            Some(token) => builder.after(token),
            None => builder,
        };
        let page = match builder.execute() {
            Ok(page) => page,
            Err(error) => {
                stop = Some(format!("page_refused: {error}"));
                break;
            }
        };
        let checked = (|| -> Result<(u64, u64), String> {
            if page.generation != *pin || page.rank_unit != TextRankUnit::File {
                return Err("page generation or rank unit differs from request".into());
            }
            if pages.is_empty()
                && (page.window != *window
                    || page.results.len() != hits.len()
                    || !page
                        .results
                        .iter()
                        .zip(hits)
                        .all(|(row, hit)| same_original_hit(row, hit)))
            {
                return Err("pinned first page differs from the measured original window".into());
            }
            let scope_count = page.explanation.planner_trace.iter().filter(|entry|
                entry.detail == "code_search.execution.scope=ordinary_exhaustive_page_v1;exploration_complete=true").count();
            if scope_count != 1 {
                return Err("ordinary exhaustive execution is not established".into());
            }
            for row in &page.results {
                if row.repo_id != pin.repo_id
                    || row.source_repo_id != pin.repo_id
                    || row.revision_id != pin.revision_id
                    || row.manifest_generation != pin.manifest_generation
                    || !row.candidate_id.starts_with("file:")
                    || row.source.is_none()
                    || row.preview.is_none()
                {
                    return Err("candidate lacks pinned source-file authority".into());
                }
            }
            let total = trace_count(&page.explanation, "verified_matching_files")?;
            let eligible = trace_count(&page.explanation, "cursor_eligible_files")?;
            if trace_count(&page.explanation, "returned_files")? != page.results.len() as u64 {
                return Err("public returned count differs from native page".into());
            }
            Ok((total, eligible))
        })();
        let page_json = serde_json::to_value(&page);
        let page_json = match page_json {
            Ok(value) => value,
            Err(error) => {
                stop = Some(format!("page_serialization: {error}"));
                break;
            }
        };
        pages.push(page_json);
        let (total, eligible) = match checked {
            Ok(counts) => counts,
            Err(error) => {
                stop = Some(error);
                break;
            }
        };
        if candidates
            .len()
            .checked_add(page.results.len())
            .is_none_or(|n| n > limits.max_files)
        {
            stop = Some("diagnostic_file_limit".into());
            break;
        }
        if let Err(error) = progress.observe(&page.results, total, eligible) {
            stop = Some(error);
            break;
        }
        candidates.extend(page.results);
        if page.window.outcome().is_exhausted() {
            if page.next_cursor.is_some() || !progress.exhausted() {
                stop = Some("exhaustion disagrees with verified file count".into());
            } else {
                pool_complete = true;
            }
            break;
        }
        let Some(next) = page.next_cursor else {
            stop = Some("non_exhausted_page_without_cursor".into());
            break;
        };
        if candidates.is_empty() || !cursor_values.insert(format!("{next:?}")) {
            stop = Some("non_progressing_cursor".into());
            break;
        }
        cursor = Some(next);
    }
    if !pool_complete && stop.is_none() {
        stop = Some("diagnostic_page_limit".into());
    }
    // A partial pool is still inspectable, but no consumer may rank it as complete.
    for candidate in candidates {
        if start.elapsed() >= limits.timeout {
            stop = Some("diagnostic_deadline".into());
            break;
        }
        let explanation_start = Instant::now();
        let response =
            client
                .search()
                .explain_under_query(pin.clone(), candidate.clone(), request.clone());
        let elapsed = explanation_start.elapsed().as_secs_f64() * 1000.0;
        let row = match response {
            Ok(response) => match serde_json::to_value(&response) {
                Ok(response) => {
                    json!({"candidate":candidate,"status":"returned", "response":response,"explanation_ms":elapsed})
                }
                Err(error) => {
                    json!({"candidate":candidate,"status":"refused","error":format!("explanation_serialization: {error}"),"explanation_ms":elapsed})
                }
            },
            Err(error) => {
                json!({"candidate":candidate,"status":"refused","error":error.to_string(),"explanation_ms":elapsed})
            }
        };
        explanations.push(row);
    }
    json!({"status":if stop.is_none() { "returned" } else { "partial" },
        "reason":stop,"pool_complete":pool_complete,"pages":pages,"explanations":explanations,
        "diagnostic_ms":start.elapsed().as_secs_f64()*1000.0})
}

pub(super) fn collect(
    client: &QuantaIndex,
    pin: &GenerationPin,
    pack: &QueryPack,
    plans: &BTreeMap<String, QueryPlan>,
    outcomes: &BTreeMap<(String, String), QueryOutcome>,
    top_k: u32,
    limits: Limits,
) -> Vec<Value> {
    pack.tasks.iter().map(|task| {
        let Some(plan) = plans.get(task.task_id.as_str()) else {
            return json!({"task_id":task.task_id,"route":"lexical","status":"not_run","reason":"missing_plan"});
        };
        let request = TextQueryRequest { syntax:TextQuerySyntax::CodeSearch,
            query_text:plan.lexical_request.clone(),constraints:QueryConstraintSetV1::unconstrained(),
            generation:Some(pin.clone()),generation_selector:None,top_k,cursor:None };
        let result = outcomes.get(&(task.task_id.clone(), "lexical".into()))
            .map(|original| collect_one(client, pin, &request, original, limits))
            .unwrap_or_else(|| json!({"status":"not_run","reason":"missing_original_outcome"}));
        json!({"task_id":task.task_id,"route":"lexical", "generation":pin,
            "effective_request":request,"query_identity":{
                "original_query_sha256":plan.original_query_sha256,
                "policy_config_sha256":plan.policy_config_sha256,
                "effective_lexical_request_sha256":plan.effective_lexical_request_sha256,
                "semantic_text_sha256":plan.semantic_text_sha256},"collection":result})
    }).collect()
}

pub(super) fn allowed(policy: QueryInputPolicy) -> bool {
    matches!(
        policy,
        QueryInputPolicy::CodeSearchFile | QueryInputPolicy::CodeSearchExactContentFile
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use quanta_index_contract::{ManifestGeneration, RepoId, RepoRelativePath, RevisionId};
    fn candidate(path: &str, score: f32) -> LexicalCandidate {
        LexicalCandidate {
            source_repo_id: RepoId::new("repo").expect("valid repo"),
            source: None,
            preview: None,
            candidate_id: format!("file:{path}"),
            repo_id: RepoId::new("repo").expect("valid repo"),
            revision_id: RevisionId::new("rev").expect("valid revision"),
            manifest_generation: ManifestGeneration::new(1),
            repo_relative_path: RepoRelativePath::new(path),
            start_line: 1,
            end_line: 1,
            score,
            snippet: String::new(),
            snippet_hit_offset: None,
            highlights: Vec::new(),
        }
    }
    #[test]
    fn full_pool_requires_stable_counts_order_and_exhaustion() {
        let mut pool = PoolProgress::default();
        pool.observe(&[candidate("z.rs", 109.0)], 3, 3)
            .expect("first page");
        assert!(!pool.exhausted(), "top-k is not the full pool");
        pool.observe(&[candidate("a.rs", 107.0), candidate("b.rs", 105.0)], 3, 2)
            .expect("continuation");
        assert!(pool.exhausted());
    }
    #[test]
    fn duplicate_reordered_and_drifting_pages_refuse() {
        for second in [candidate("z.rs", 109.0), candidate("a.rs", 110.0)] {
            let mut pool = PoolProgress::default();
            pool.observe(&[candidate("z.rs", 109.0)], 2, 2)
                .expect("first");
            assert!(pool.observe(&[second], 2, 1).is_err());
        }
        let mut pool = PoolProgress::default();
        pool.observe(&[candidate("z.rs", 109.0)], 2, 2)
            .expect("first");
        assert!(pool.observe(&[candidate("a.rs", 107.0)], 3, 2).is_err());
    }
    #[test]
    fn explicit_recovery_and_non_code_search_policies_are_not_ordinary_studies() {
        assert!(allowed(QueryInputPolicy::CodeSearchFile));
        assert!(allowed(QueryInputPolicy::CodeSearchExactContentFile));
        for policy in [
            QueryInputPolicy::CodeSearchTypoFile,
            QueryInputPolicy::CodeSearchComponentsFile,
            QueryInputPolicy::SubstringFile,
            QueryInputPolicy::KeywordFile,
        ] {
            assert!(!allowed(policy));
        }
    }
}
