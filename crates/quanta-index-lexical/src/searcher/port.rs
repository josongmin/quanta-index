//! The `LexicalSearcher` port over a sealed generation.

use crate::documents::stored_text;
use crate::ranked_page::{ProjectionGroup, rank_in_memory};
use crate::searcher::planner_errors::planner_preflight_expr;
use crate::searcher::query_rewrite::rewrite_symbol_name_predicate_query;
use crate::searcher::snippets::snippet_center_terms;
use crate::{ManualPage, QueryDocKind, TEXT_DOC_KIND, TantivySearcher};
use quanta_index_contract::{
    CandidatePresenceV1, FileOwnerProjectionRow, LexicalCandidate, LqQuery, QueryConstraintSetV1,
    RepoId, SymbolCandidate,
};
use quanta_index_core::{
    CoreError, LexicalArtifactIdentityV1, LexicalCandidateExplanationV1, LexicalPageSpec,
    LexicalScoreEngineV1, LexicalScoreTraceV1, LexicalSearchPageV1, LexicalSearcher,
    RequestBudgetV1, domains::lexical::LexicalPolicy,
};
use std::collections::BTreeSet;
use tantivy::Term;
use tantivy::collector::TopDocs;
use tantivy::query::TermQuery;
use tantivy::schema::{IndexRecordOption, TantivyDocument};

impl LexicalSearcher for TantivySearcher {
    fn resident_bytes_estimate(&self) -> u64 {
        self.resident_bytes_estimate
    }

    fn artifact_identity(&self) -> LexicalArtifactIdentityV1 {
        self.artifact_identity.clone()
    }

    fn search_constrained(
        &self,
        query: &LqQuery,
        constraints: &QueryConstraintSetV1,
        page: &LexicalPageSpec,
        budget: &RequestBudgetV1,
    ) -> Result<LexicalSearchPageV1, CoreError> {
        // This is the single text-query execution path. The unconstrained
        // port method delegates here so constraint support cannot drift into
        // a second planner/search implementation.
        let effective_query =
            rewrite_symbol_name_predicate_query(query)?.unwrap_or_else(|| query.clone());
        LexicalPolicy::validate_query_with_constraints(&effective_query, constraints)?;
        let after = self.page_boundary(page)?;
        let empty_page = || LexicalSearchPageV1 {
            candidates: Vec::new(),
            exact_total: Self::wants_exact_total(&effective_query).then_some(0),
        };
        let Some(prepared_query) =
            self.prepare_executable_query(&effective_query, QueryDocKind::Text, budget)?
        else {
            return Ok(empty_page());
        };
        planner_preflight_expr(
            &prepared_query.query,
            &prepared_query.predicate_plan.expr,
            self.repo_metadata.is_some(),
        )?;
        if !self.repo_filters_allow(&effective_query)? {
            return Ok(empty_page());
        }
        let requested = usize::try_from(page.fetch)
            .map_err(|err| CoreError::InvalidContract(format!("lexical: page fetch: {err}")))?;
        let limit = Self::page_limit(&effective_query, requested);
        if limit == 0 {
            return Ok(empty_page());
        }
        let group = Self::projection_group(&effective_query);
        if Self::uses_unindexed_scan(&effective_query.options) {
            Self::ensure_manual_scan_supports_constraints(constraints, "lexical")?;
            return self.manual_text_search(
                &effective_query,
                &prepared_query,
                constraints,
                &ManualPage {
                    limit,
                    after: after.as_deref(),
                    group,
                },
                budget,
            );
        }
        let Some(base) = self.compile_query_with_constraints(
            &prepared_query.query,
            &prepared_query.predicate_plan,
            constraints,
            budget,
        )?
        else {
            return Ok(empty_page());
        };
        let compiled =
            self.with_doc_kind_and_constraints(base, prepared_query.doc_kind.as_str(), constraints);
        let boost = Self::boost_factor(&effective_query.options);
        let searcher = self.reader.searcher();
        // A projection's row universe is its groups, so its total is exact
        // whether or not a count was asked for; a plain page counts only
        // when asked.
        let (rows, exact_total) = if let Some(group) = group {
            let (rows, total) = self.collect_projection(
                &searcher,
                &*compiled,
                group,
                after.as_deref(),
                boost,
                limit,
                "projected text search",
                budget,
            )?;
            (rows, Some(total))
        } else {
            let counts = Self::wants_exact_total(&effective_query);
            let fruit = self.collect_ranked_page(
                &searcher,
                &*compiled,
                limit,
                after,
                boost,
                counts,
                "text search",
                budget,
            )?;
            (fruit.rows, counts.then_some(fruit.matched))
        };
        let center_terms = snippet_center_terms(&effective_query);
        Ok(LexicalSearchPageV1 {
            candidates: self.rows_to_candidates(
                &searcher,
                rows,
                &center_terms,
                Self::document_to_candidate,
            )?,
            exact_total,
        })
    }

    fn project_file_owners(
        &self,
        candidates: &[LexicalCandidate],
    ) -> Result<Vec<FileOwnerProjectionRow>, CoreError> {
        let authority = self.file_ownership_authority()?;
        let searcher = self.reader.searcher();
        let mut rows: Vec<FileOwnerProjectionRow> = Vec::with_capacity(candidates.len());
        for candidate in candidates {
            let source_repo_hit = searcher
                .search(
                    &TermQuery::new(
                        Term::from_field_text(
                            self.fields.candidate_id,
                            candidate.candidate_id.as_str(),
                        ),
                        IndexRecordOption::Basic,
                    ),
                    &TopDocs::with_limit(1),
                )
                .map_err(|err| {
                    CoreError::Storage(format!(
                        "lexical: file owner projection candidate lookup `{}`: {err}",
                        candidate.candidate_id
                    ))
                })?
                .into_iter()
                .next();
            // A storage error fetching the matched doc propagates (fail-closed);
            // a missing repo_id field falls back to the candidate's own repo_id,
            // which is the authoritative value the candidate already carries.
            let source_repo_id = match source_repo_hit {
                Some((_score, doc_address)) => {
                    let doc = searcher
                        .doc::<TantivyDocument>(doc_address)
                        .map_err(|err| {
                            CoreError::Storage(format!(
                                "lexical: file owner projection doc fetch `{}`: {err}",
                                candidate.candidate_id
                            ))
                        })?;
                    stored_text(&doc, self.fields.repo_id)
                        .unwrap_or_else(|| candidate.repo_id.as_str().to_string())
                }
                None => candidate.repo_id.as_str().to_string(),
            };
            let owners = authority
                .owners_by_repo_id
                .get(&source_repo_id)
                .and_then(|by_path| by_path.get(candidate.repo_relative_path.as_str()))
                .map(|set| set.iter().cloned().collect::<Vec<_>>())
                .unwrap_or_default();
            rows.push(FileOwnerProjectionRow {
                candidate_id: candidate.candidate_id.clone(),
                repo_id: RepoId::new(source_repo_id),
                revision_id: candidate.revision_id.clone(),
                manifest_generation: candidate.manifest_generation,
                repo_relative_path: candidate.repo_relative_path.clone(),
                owners,
            });
        }
        Ok(rows)
    }

    fn search_symbols(
        &self,
        query: &LqQuery,
        top_k: u32,
        budget: &RequestBudgetV1,
    ) -> Result<Vec<SymbolCandidate>, CoreError> {
        self.search_symbols_constrained(
            query,
            &QueryConstraintSetV1::unconstrained(),
            &LexicalPageSpec::first(top_k),
            budget,
        )
    }

    fn search_symbols_constrained(
        &self,
        query: &LqQuery,
        constraints: &QueryConstraintSetV1,
        page: &LexicalPageSpec,
        budget: &RequestBudgetV1,
    ) -> Result<Vec<SymbolCandidate>, CoreError> {
        // Single symbol-query execution path; the unconstrained entrypoint
        // delegates here to prevent planner and scoring drift.
        let effective_query =
            rewrite_symbol_name_predicate_query(query)?.unwrap_or_else(|| query.clone());
        LexicalPolicy::validate_query_with_constraints(&effective_query, constraints)?;
        let after = self.page_boundary(page)?;
        let Some(prepared_query) =
            self.prepare_executable_query(&effective_query, QueryDocKind::Symbol, budget)?
        else {
            return Ok(Vec::new());
        };
        planner_preflight_expr(
            &prepared_query.query,
            &prepared_query.predicate_plan.expr,
            self.repo_metadata.is_some(),
        )?;
        if !self.repo_filters_allow(&effective_query)? {
            return Ok(Vec::new());
        }
        let requested = usize::try_from(page.fetch)
            .map_err(|err| CoreError::InvalidContract(format!("symbol: page fetch: {err}")))?;
        let limit = Self::page_limit(&effective_query, requested);
        if limit == 0 {
            return Ok(Vec::new());
        }
        if Self::uses_unindexed_scan(&effective_query.options) {
            Self::ensure_manual_scan_supports_constraints(constraints, "symbol")?;
            let mut rows = rank_in_memory(
                self.manual_symbol_matches(&effective_query, &prepared_query, constraints, budget)?,
                after.as_deref(),
            );
            rows.truncate(limit);
            return Ok(rows);
        }
        let Some(base) = self.compile_query_with_constraints(
            &prepared_query.query,
            &prepared_query.predicate_plan,
            constraints,
            budget,
        )?
        else {
            return Ok(Vec::new());
        };
        let compiled =
            self.with_doc_kind_and_constraints(base, prepared_query.doc_kind.as_str(), constraints);
        let searcher = self.reader.searcher();
        let fruit = self.collect_ranked_page(
            &searcher,
            &*compiled,
            limit,
            after,
            Self::boost_factor(&effective_query.options),
            false,
            "symbol search",
            budget,
        )?;
        let center_terms = snippet_center_terms(&effective_query);
        self.rows_to_candidates(
            &searcher,
            fruit.rows,
            &center_terms,
            Self::document_to_symbol_candidate,
        )
    }

    fn search_symbols_all(
        &self,
        query: &LqQuery,
        budget: &RequestBudgetV1,
    ) -> Result<Vec<SymbolCandidate>, CoreError> {
        let effective_query =
            rewrite_symbol_name_predicate_query(query)?.unwrap_or_else(|| query.clone());
        LexicalPolicy::validate_query(&effective_query)?;
        let Some(prepared_query) =
            self.prepare_executable_query(&effective_query, QueryDocKind::Symbol, budget)?
        else {
            return Ok(Vec::new());
        };
        planner_preflight_expr(
            &prepared_query.query,
            &prepared_query.predicate_plan.expr,
            self.repo_metadata.is_some(),
        )?;
        if !self.repo_filters_allow(&effective_query)? {
            return Ok(Vec::new());
        }
        let searcher = self.reader.searcher();
        let requested = Self::corpus_docs(&searcher, "symbol scope materialization")?;
        let limit = Self::page_limit(&effective_query, requested);
        if limit == 0 {
            return Ok(Vec::new());
        }
        if Self::uses_unindexed_scan(&effective_query.options) {
            let mut rows = rank_in_memory(
                self.manual_symbol_matches(
                    &effective_query,
                    &prepared_query,
                    &QueryConstraintSetV1::unconstrained(),
                    budget,
                )?,
                None,
            );
            rows.truncate(limit);
            return Ok(rows);
        }
        let Some(base) = self.compile_query_from_prepared(
            &prepared_query.query,
            &prepared_query.predicate_plan,
            budget,
        )?
        else {
            return Ok(Vec::new());
        };
        let compiled = self.with_doc_kind(base, prepared_query.doc_kind.as_str());
        let mut rows = self.collect_whole_set(
            &searcher,
            &*compiled,
            Self::boost_factor(&effective_query.options),
            "symbol scope materialization",
            budget,
        )?;
        rows.truncate(limit);
        let center_terms = snippet_center_terms(&effective_query);
        self.rows_to_candidates(
            &searcher,
            rows,
            &center_terms,
            Self::document_to_symbol_candidate,
        )
    }

    fn search_all(
        &self,
        query: &LqQuery,
        budget: &RequestBudgetV1,
    ) -> Result<Vec<LexicalCandidate>, CoreError> {
        let constraints = &QueryConstraintSetV1::unconstrained();
        LexicalPolicy::validate_query_with_constraints(query, constraints)?;
        let Some(prepared_query) =
            self.prepare_executable_query(query, QueryDocKind::Text, budget)?
        else {
            return Ok(Vec::new());
        };
        planner_preflight_expr(
            &prepared_query.query,
            &prepared_query.predicate_plan.expr,
            self.repo_metadata.is_some(),
        )?;
        if !self.repo_filters_allow(query)? {
            return Ok(Vec::new());
        }
        let searcher = self.reader.searcher();
        let requested = Self::corpus_docs(&searcher, "structural scope materialization")?;
        let limit = Self::page_limit(query, requested);
        if limit == 0 {
            return Ok(Vec::new());
        }
        // Structural routing keeps every chunk hit; only `select:repo`
        // collapses them, to the generation's first row.
        let repo_only = Self::projects_repo_surface(query);
        if Self::uses_unindexed_scan(&query.options) {
            Self::ensure_manual_scan_supports_constraints(constraints, "lexical")?;
            let page = self.manual_text_search(
                query,
                &prepared_query,
                constraints,
                &ManualPage {
                    limit,
                    after: None,
                    group: repo_only.then_some(ProjectionGroup::Repo),
                },
                budget,
            )?;
            return Ok(page.candidates);
        }
        let Some(base) = self.compile_query_with_constraints(
            &prepared_query.query,
            &prepared_query.predicate_plan,
            constraints,
            budget,
        )?
        else {
            return Ok(Vec::new());
        };
        let compiled =
            self.with_doc_kind_and_constraints(base, prepared_query.doc_kind.as_str(), constraints);
        let mut rows = self.collect_whole_set(
            &searcher,
            &*compiled,
            Self::boost_factor(&query.options),
            "structural scope materialization",
            budget,
        )?;
        rows.truncate(if repo_only { limit.min(1) } else { limit });
        let center_terms = snippet_center_terms(query);
        self.rows_to_candidates(&searcher, rows, &center_terms, Self::document_to_candidate)
    }

    fn candidate_presence(&self, candidate_id: &str) -> Result<CandidatePresenceV1, CoreError> {
        let searcher = self.reader.searcher();
        Ok(
            match self.locate_candidate(&searcher, candidate_id, TEXT_DOC_KIND)? {
                Some(_) => CandidatePresenceV1::Indexed,
                None => CandidatePresenceV1::NotIndexed,
            },
        )
    }

    fn explain_candidate(
        &self,
        query: &LqQuery,
        constraints: &QueryConstraintSetV1,
        candidate_id: &str,
        budget: &RequestBudgetV1,
    ) -> Result<LexicalCandidateExplanationV1, CoreError> {
        // The same preparation as `search_constrained`, step for step, so
        // the plan that scores this one document is the plan that ranked it.
        let effective_query =
            rewrite_symbol_name_predicate_query(query)?.unwrap_or_else(|| query.clone());
        LexicalPolicy::validate_query_with_constraints(&effective_query, constraints)?;
        let searcher = self.reader.searcher();
        let not_matched = |reason: &str| {
            Ok(LexicalCandidateExplanationV1::NotMatched {
                reason: reason.to_string(),
            })
        };
        let Some(prepared_query) =
            self.prepare_executable_query(&effective_query, QueryDocKind::Text, budget)?
        else {
            return match self.locate_candidate(&searcher, candidate_id, TEXT_DOC_KIND)? {
                Some(_) => not_matched("the plan matches no document"),
                None => Ok(LexicalCandidateExplanationV1::NotIndexed),
            };
        };
        let doc_kind = prepared_query.doc_kind.as_str();
        let Some((doc_address, doc)) = self.locate_candidate(&searcher, candidate_id, doc_kind)?
        else {
            return Ok(LexicalCandidateExplanationV1::NotIndexed);
        };
        planner_preflight_expr(
            &prepared_query.query,
            &prepared_query.predicate_plan.expr,
            self.repo_metadata.is_some(),
        )?;
        if !self.repo_filters_allow(&effective_query)? {
            return not_matched("a repo filter excludes this generation");
        }
        let boost_factor = Self::boost_factor(&effective_query.options);
        if Self::uses_unindexed_scan(&effective_query.options) {
            Self::ensure_manual_scan_supports_constraints(constraints, "lexical")?;
            if !self.manual_doc_matches(
                &doc,
                &effective_query,
                &prepared_query,
                constraints,
                budget,
            )? {
                return not_matched("the unindexed scan does not match the document");
            }
            return Ok(LexicalCandidateExplanationV1::Matched(
                LexicalScoreTraceV1 {
                    engine: LexicalScoreEngineV1::UnindexedScan,
                    engine_score: 1.0,
                    boost_factor,
                    emitted_score: Self::apply_query_boost_score(1.0, &effective_query.options),
                },
            ));
        }
        let Some(base) = self.compile_query_with_constraints(
            &prepared_query.query,
            &prepared_query.predicate_plan,
            constraints,
            budget,
        )?
        else {
            return not_matched("the plan compiles to nothing");
        };
        let compiled = self.with_doc_kind_and_constraints(base, doc_kind, constraints);
        let Some(engine_score) = Self::score_one_document(&searcher, &*compiled, doc_address)?
        else {
            return not_matched("the plan does not match the document");
        };
        Ok(LexicalCandidateExplanationV1::Matched(
            LexicalScoreTraceV1 {
                engine: LexicalScoreEngineV1::Bm25,
                engine_score,
                boost_factor,
                emitted_score: Self::apply_query_boost_score(
                    engine_score,
                    &effective_query.options,
                ),
            },
        ))
    }

    fn admitted_candidates(
        &self,
        query: &LqQuery,
        constraints: &QueryConstraintSetV1,
        candidate_ids: &BTreeSet<String>,
        budget: &RequestBudgetV1,
    ) -> Result<BTreeSet<String>, CoreError> {
        self.admitted_candidates_v1(query, constraints, candidate_ids, budget)
    }
}
