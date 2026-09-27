//! Exact per-candidate filter admission for the hybrid dense lane
//! (QI-BB-018 보완 #3).
//!
//! The dense lane ranks vectors and cannot compile the DSL, so the filters
//! the hybrid contract classes as `exact` are evaluated here, through the
//! same preparation and compilation `search_constrained` runs: the
//! filter-only plan is prepared step for step as a ranked search would be
//! (symbol-name rewrite, doc-kind routing, predicate plan, planner
//! pre-flight, the generation-level repo gate), restricted to the dense
//! candidate ids, and collected once. A candidate the plan admits is a live
//! document of the plan's doc kind that the compiled filters match; every
//! other candidate — not indexed here, another doc kind, excluded by a
//! filter — is not admitted. The restriction is by exact candidate-id
//! terms, so the answer does not depend on the corpus around the
//! candidates, and the collect is bounded by the candidate count.

use std::collections::BTreeSet;

use quanta_index_contract::{LqQuery, QueryConstraintSetV1};
use quanta_index_core::{CoreError, LexicalPolicy, RequestBudgetV1};
use tantivy::TantivyDocument;

use crate::TantivySearcher;
use crate::documents::required_stored_text;
use crate::searcher::manual_scan::ManualScanCache;
use crate::searcher::planner_errors::planner_preflight_expr;
use crate::searcher::query_rewrite::rewrite_symbol_name_predicate_query;

/// The budget checkpoint and collect stage this evaluation reports under.
const ADMISSION_STAGE: &str = "lexical:admission";

impl TantivySearcher {
    /// [`quanta_index_core::LexicalSearcher::admitted_candidates`] for the
    /// Tantivy adapter.
    pub(crate) fn admitted_candidates_v1(
        &self,
        query: &LqQuery,
        constraints: &QueryConstraintSetV1,
        candidate_ids: &BTreeSet<String>,
        budget: &RequestBudgetV1,
    ) -> Result<BTreeSet<String>, CoreError> {
        if candidate_ids.is_empty() {
            return Ok(BTreeSet::new());
        }
        budget.checkpoint(ADMISSION_STAGE)?;
        if candidate_ids.len() > self.execution_budget.max_examined_candidates() {
            return Err(self.execution_budget.exceeded("dense candidate admission"));
        }
        // The same preparation as `search_constrained`, step for step, so
        // the plan that admits a dense candidate is the plan that would
        // have ranked it on the lexical lane.
        let effective_query =
            rewrite_symbol_name_predicate_query(query)?.unwrap_or_else(|| query.clone());
        let plan = LexicalPolicy::plan_candidate_filter_query(
            &effective_query,
            constraints,
            candidate_ids,
        )?;
        let Some(mut prepared) = self.prepare_executable_query(&plan, budget)? else {
            // The predicate plan proved the filters admit nothing in this
            // generation (a repo or path scope with no members).
            return Ok(BTreeSet::new());
        };
        planner_preflight_expr(
            &prepared.query,
            &prepared.predicate_plan.expr,
            self.repo_metadata.is_some(),
        )?;
        if !self.repo_filters_allow(&effective_query)? {
            // A generation-level gate (`fork:`, `archived:`, `visibility:`,
            // `context:`) excludes every document of this generation.
            return Ok(BTreeSet::new());
        }
        let searcher = self.reader.searcher();
        if Self::uses_unindexed_scan(&effective_query.options) {
            Self::ensure_manual_scan_supports_constraints(constraints, "lexical")?;
            self.ensure_manual_language_query_supported(&effective_query)?;
            let mut admitted = BTreeSet::new();
            let mut regex_cache = ManualScanCache::default();
            for candidate_id in candidate_ids {
                budget.checkpoint(ADMISSION_STAGE)?;
                let Some((_address, doc)) =
                    self.locate_candidate(&searcher, candidate_id, prepared.doc_kind.as_str())?
                else {
                    continue;
                };
                if self.manual_doc_matches(
                    &doc,
                    &effective_query,
                    &prepared,
                    constraints,
                    budget,
                    &mut regex_cache,
                )? {
                    let _first = admitted.insert(candidate_id.clone());
                }
            }
            return Ok(admitted);
        }
        // Restrict the compiled filters to exactly the dense candidates:
        // one bounded collect, never a ranked re-search of the corpus.
        prepared.predicate_plan.allowed_candidate_ids = Some(candidate_ids.clone());
        let Some(base) =
            self.compile_query_from_prepared(&prepared.query, &prepared.predicate_plan, budget)?
        else {
            return Ok(BTreeSet::new());
        };
        let compiled =
            self.with_doc_kind_and_constraints(base, prepared.doc_kind.as_str(), constraints);
        // Whole-set collection charges native work and retained bytes before
        // decoding; an extra row or duplicate id cannot become a partial
        // admission result.
        let hits = self.collect_whole_set(
            &searcher,
            &*compiled,
            1.0,
            "dense candidate admission",
            budget,
        )?;
        let mut admitted = BTreeSet::new();
        for row in hits {
            budget.checkpoint(ADMISSION_STAGE)?;
            let doc_address = row.address;
            let doc: TantivyDocument = searcher.doc(doc_address).map_err(|err| {
                CoreError::Storage(format!(
                    "lexical: admission fetch doc {doc_address:?}: {err}"
                ))
            })?;
            let candidate_id =
                required_stored_text(&doc, self.fields.candidate_id, "candidate_id")?;
            if !candidate_ids.contains(candidate_id) {
                return Err(CoreError::Storage(format!(
                    "lexical: admission matched `{candidate_id}`, which is not a dense candidate"
                )));
            }
            if !admitted.insert(candidate_id.to_owned()) {
                return Err(CoreError::Storage(format!(
                    "lexical: candidate id `{candidate_id}` names 2 live documents"
                )));
            }
        }
        Ok(admitted)
    }
}
