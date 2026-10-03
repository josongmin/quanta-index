//! The `LexicalSearcher` port over a sealed generation.

use crate::ranked_page::{ProjectionGroup, rank_in_memory};
use crate::searcher::planner_errors::{planner_preflight_expr, validate_exact_all_count};
use crate::searcher::query_rewrite::rewrite_symbol_name_predicate_query;
use crate::{ManualPage, SYMBOL_DOC_KIND, TEXT_DOC_KIND, TantivySearcher};
use quanta_index_contract::{
    CandidatePresenceV1, FileOwnerProjectionRow, LexicalCandidate, LqPatternType, LqQuery,
    QueryConstraintSetV1, SymbolCandidate,
};
use quanta_index_core::{
    CoreError, LexicalArtifactIdentityV1, LexicalCandidateExplanationV1, LexicalEndpoint,
    LexicalPageSpec, LexicalScoreEngineV1, LexicalScoreTraceV1, LexicalSearchPageV1,
    LexicalSearcher, RequestBudgetV1, SymbolSearchPageV1, domains::lexical::LexicalPolicy,
    validate_internal_fetch_size,
};
use std::collections::BTreeSet;

impl LexicalSearcher for TantivySearcher {
    fn resident_bytes_estimate(&self) -> u64 {
        self.resident_bytes_estimate
    }

    fn artifact_identity(&self) -> LexicalArtifactIdentityV1 {
        self.artifact_identity.clone()
    }

    fn source_file_coverage(&self) -> Option<&quanta_index_contract::FileCoverageSnapshot> {
        self.source_coverage.as_ref()
    }

    fn source_publication_event(&self) -> Option<&quanta_index_contract::SourcePublicationEvent> {
        self.source_publication_event.as_ref()
    }

    fn search_constrained(
        &self,
        query: &LqQuery,
        constraints: &QueryConstraintSetV1,
        page: &LexicalPageSpec,
        budget: &RequestBudgetV1,
    ) -> Result<LexicalSearchPageV1, CoreError> {
        if query.options.pattern_type == LqPatternType::CodeSearch {
            let _plan = LexicalPolicy::plan_query(query, constraints, LexicalEndpoint::Text)?;
            let _fetch = validate_internal_fetch_size(page.fetch)?;
            return self.search_code_files(query, constraints, page, budget);
        }
        // This is the single text-query execution path. The unconstrained
        // port method delegates here so constraint support cannot drift into
        // a second planner/search implementation.
        let effective_query =
            rewrite_symbol_name_predicate_query(query)?.unwrap_or_else(|| query.clone());
        let plan = LexicalPolicy::plan_query(&effective_query, constraints, LexicalEndpoint::Text)?;
        let _accepted_fetch = validate_internal_fetch_size(page.fetch)?;
        let after = self.page_boundary(page)?;
        let empty_page = || LexicalSearchPageV1 {
            candidates: Vec::new(),
            exact_total: Self::wants_exact_total(&effective_query).then_some(0),
        };
        let Some(prepared_query) = self.prepare_executable_query(&plan, budget)? else {
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
        let mut preview = self.selected_preview_context(
            &effective_query,
            &prepared_query.predicate_plan,
            budget,
        )?;
        Ok(LexicalSearchPageV1 {
            candidates: self.rows_to_candidates(
                &searcher,
                rows,
                &mut preview,
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
        let mut rows: Vec<FileOwnerProjectionRow> = Vec::with_capacity(candidates.len());
        for candidate in candidates {
            let owners = authority
                .owners_by_repo_id
                .get(candidate.source_repo_id.as_str())
                .and_then(|by_path| by_path.get(candidate.repo_relative_path.as_str()))
                .map(|set| set.iter().cloned().collect::<Vec<_>>())
                .unwrap_or_default();
            rows.push(FileOwnerProjectionRow {
                candidate_id: candidate.candidate_id.clone(),
                source_repo_id: candidate.source_repo_id.clone(),
                // The source repo selects the ownership authority. The
                // projection identity must still pair with the ranked row.
                repo_id: candidate.repo_id.clone(),
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
        .map(|page| page.candidates)
    }

    fn search_symbols_constrained(
        &self,
        query: &LqQuery,
        constraints: &QueryConstraintSetV1,
        page: &LexicalPageSpec,
        budget: &RequestBudgetV1,
    ) -> Result<SymbolSearchPageV1, CoreError> {
        // Single symbol-query execution path; the unconstrained entrypoint
        // delegates here to prevent planner and scoring drift.
        let effective_query =
            rewrite_symbol_name_predicate_query(query)?.unwrap_or_else(|| query.clone());
        let plan =
            LexicalPolicy::plan_query(&effective_query, constraints, LexicalEndpoint::Symbol)?;
        let _accepted_fetch = validate_internal_fetch_size(page.fetch)?;
        let after = self.page_boundary(page)?;
        let Some(prepared_query) = self.prepare_executable_query(&plan, budget)? else {
            return Ok(SymbolSearchPageV1 {
                candidates: Vec::new(),
                exact_total: Some(0),
            });
        };
        planner_preflight_expr(
            &prepared_query.query,
            &prepared_query.predicate_plan.expr,
            self.repo_metadata.is_some(),
        )?;
        if !self.repo_filters_allow(&effective_query)? {
            return Ok(SymbolSearchPageV1 {
                candidates: Vec::new(),
                exact_total: Some(0),
            });
        }
        let requested = usize::try_from(page.fetch)
            .map_err(|err| CoreError::InvalidContract(format!("symbol: page fetch: {err}")))?;
        let limit = Self::page_limit(&effective_query, requested);
        if Self::uses_unindexed_scan(&effective_query.options) {
            Self::ensure_manual_scan_supports_constraints(constraints, "symbol")?;
            let mut rows = rank_in_memory(
                self.manual_symbol_matches(&effective_query, &prepared_query, constraints, budget)?,
                after.as_deref(),
            );
            let exact_total = Some(crate::channel_payloads::count_from_len(rows.len())?);
            rows.truncate(limit);
            let mut preview = self.selected_preview_context(
                &effective_query,
                &prepared_query.predicate_plan,
                budget,
            )?;
            return Ok(SymbolSearchPageV1 {
                candidates: self.render_manual_candidates(
                    rows,
                    &mut preview,
                    Self::document_to_symbol_candidate,
                )?,
                exact_total,
            });
        }
        let Some(base) = self.compile_query_with_constraints(
            &prepared_query.query,
            &prepared_query.predicate_plan,
            constraints,
            budget,
        )?
        else {
            return Ok(SymbolSearchPageV1 {
                candidates: Vec::new(),
                exact_total: Some(0),
            });
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
            Self::wants_exact_total(&effective_query),
            "symbol search",
            budget,
        )?;
        let mut preview = self.selected_preview_context(
            &effective_query,
            &prepared_query.predicate_plan,
            budget,
        )?;
        Ok(SymbolSearchPageV1 {
            candidates: self.rows_to_candidates(
                &searcher,
                fruit.rows,
                &mut preview,
                Self::document_to_symbol_candidate,
            )?,
            exact_total: Self::wants_exact_total(&effective_query).then_some(fruit.matched),
        })
    }

    fn search_symbols_all(
        &self,
        query: &LqQuery,
        budget: &RequestBudgetV1,
    ) -> Result<Vec<SymbolCandidate>, CoreError> {
        let effective_query =
            rewrite_symbol_name_predicate_query(query)?.unwrap_or_else(|| query.clone());
        let plan = LexicalPolicy::plan_query(
            &effective_query,
            &QueryConstraintSetV1::unconstrained(),
            LexicalEndpoint::Symbol,
        )?;
        validate_exact_all_count(&effective_query)?;
        let Some(prepared_query) = self.prepare_executable_query(&plan, budget)? else {
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
            let mut preview = self.selected_preview_context(
                &effective_query,
                &prepared_query.predicate_plan,
                budget,
            )?;
            return self.render_manual_candidates(
                rows,
                &mut preview,
                Self::document_to_symbol_candidate,
            );
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
        let mut preview = self.selected_preview_context(
            &effective_query,
            &prepared_query.predicate_plan,
            budget,
        )?;
        self.rows_to_candidates(
            &searcher,
            rows,
            &mut preview,
            Self::document_to_symbol_candidate,
        )
    }

    fn search_all(
        &self,
        query: &LqQuery,
        budget: &RequestBudgetV1,
    ) -> Result<Vec<LexicalCandidate>, CoreError> {
        let effective_query =
            rewrite_symbol_name_predicate_query(query)?.unwrap_or_else(|| query.clone());
        let query = &effective_query;
        let constraints = &QueryConstraintSetV1::unconstrained();
        let plan = LexicalPolicy::plan_query(query, constraints, LexicalEndpoint::Text)?;
        validate_exact_all_count(query)?;
        let Some(prepared_query) = self.prepare_executable_query(&plan, budget)? else {
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
        // Structural routing keeps every chunk hit; repo projection keeps
        // the best representative of each source repository in the generation.
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
        let rows = if repo_only {
            self.collect_projection(
                &searcher,
                &*compiled,
                ProjectionGroup::Repo,
                None,
                Self::boost_factor(&query.options),
                limit,
                "structural repo scope materialization",
                budget,
            )?
            .0
        } else {
            let mut rows = self.collect_whole_set(
                &searcher,
                &*compiled,
                Self::boost_factor(&query.options),
                "structural scope materialization",
                budget,
            )?;
            rows.truncate(limit);
            rows
        };
        let mut preview =
            self.selected_preview_context(query, &prepared_query.predicate_plan, budget)?;
        self.rows_to_candidates(&searcher, rows, &mut preview, Self::document_to_candidate)
    }

    fn candidate_presence(&self, candidate_id: &str) -> Result<CandidatePresenceV1, CoreError> {
        if candidate_id.starts_with("file:")
            && self.code_search_file_by_id(candidate_id, &RequestBudgetV1::unbounded())?.is_some() {
            return Ok(CandidatePresenceV1::Indexed);
        }
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
        if query.options.pattern_type == LqPatternType::CodeSearch {
            let _plan = LexicalPolicy::plan_query(query, constraints, LexicalEndpoint::Text)?;
            return self.explain_code_file(query, constraints, candidate_id, budget);
        }
        // The same preparation as `search_constrained`, step for step, so
        // the plan that scores this one document is the plan that ranked it.
        let effective_query =
            rewrite_symbol_name_predicate_query(query)?.unwrap_or_else(|| query.clone());
        let plan = LexicalPolicy::plan_query(&effective_query, constraints, LexicalEndpoint::Text)?;
        let searcher = self.reader.searcher();
        let not_matched = |reason: &str| {
            Ok(LexicalCandidateExplanationV1::NotMatched {
                reason: reason.to_string(),
            })
        };
        let Some(prepared_query) = self.prepare_executable_query(&plan, budget)? else {
            let doc_kind = if plan.executes_symbol_domain() {
                SYMBOL_DOC_KIND
            } else {
                TEXT_DOC_KIND
            };
            return match self.locate_candidate(&searcher, candidate_id, doc_kind)? {
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
            self.ensure_manual_language_query_supported(&effective_query)?;
            let mut regex_cache = super::manual_scan::ManualScanCache::new(
                self.execution_budget.max_collection_bytes(),
            )?;
            if !self.manual_doc_matches(
                &doc,
                &effective_query,
                &prepared_query,
                constraints,
                budget,
                &mut regex_cache,
            )? {
                return not_matched("the unindexed scan does not match the document");
            }
            return Ok(LexicalCandidateExplanationV1::Matched(
                LexicalScoreTraceV1 {
                    engine: LexicalScoreEngineV1::UnindexedScan,
                    code_search_components: None,
                    code_search_rank_study: None,
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
                code_search_components: None,
                code_search_rank_study: None,
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
