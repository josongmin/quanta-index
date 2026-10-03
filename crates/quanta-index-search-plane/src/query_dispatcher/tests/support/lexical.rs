//! Lexical opener / searcher test doubles.

use std::collections::BTreeSet;
use std::sync::{Arc, Mutex};

use quanta_index_contract::{
    CandidatePresenceV1, GenerationSnapshot, LexicalCandidate, LqQuery, ManifestGeneration,
    QueryConstraintSetV1, RepoId, RepoRelativePath, RevisionId, SymbolCandidate,
};
use quanta_index_core::{
    CoreError, LexicalArtifactIdentityV1, LexicalCandidateExplanationV1, LexicalIndexOpenPort,
    LexicalPageSpec, LexicalScoreEngineV1, LexicalSearchPageV1, LexicalSearcher,
    RepoMetadataAuthoritiesV1, RequestBudgetV1, TextNormalizerVersionV1,
};

use crate::query_dispatcher::tests::support::common::symbol_candidate;

/// The identity a test double reports: a fixed digest and normalizer,
/// and the source-repo metadata authorities the double claims to hold.
pub(crate) fn stub_artifact_identity(
    repo_metadata: RepoMetadataAuthoritiesV1,
) -> LexicalArtifactIdentityV1 {
    LexicalArtifactIdentityV1 {
        manifest_digest: "manifest-digest-9".to_string(),
        normalizer: TextNormalizerVersionV1 { major: 2, minor: 0 },
        repo_metadata,
    }
}

pub(crate) struct RejectLexicalOpener;

impl LexicalIndexOpenPort for RejectLexicalOpener {
    fn preflight_query_primitives(
        &self,
        plan: &quanta_index_core::ValidatedLexicalPlan,
        budget: &quanta_index_core::RequestBudgetV1,
    ) -> Result<(), CoreError> {
        quanta_index_lexical::planner::LexicalPlanner::validate_query_primitives(
            plan,
            &quanta_index_lexical::regex::RegexPolicy::defaults(),
            budget,
        )
    }

    fn open(
        &self,
        _repo: &RepoId,
        _revision: &RevisionId,
        _generation: ManifestGeneration,
        _budget: &quanta_index_core::RequestBudgetV1,
    ) -> Result<Box<dyn LexicalSearcher>, CoreError> {
        Err(CoreError::NotImplemented(
            "repo-map dispatch should not open lexical index".to_string(),
        ))
    }

    fn open_proven(
        &self,
        candidate: &GenerationSnapshot,
    ) -> Result<Box<dyn LexicalSearcher>, CoreError> {
        self.open(
            &candidate.repo_id,
            &candidate.revision_id,
            candidate.manifest_generation,
            &quanta_index_core::RequestBudgetV1::unbounded(),
        )
    }
}

#[derive(Default)]
pub(crate) struct StubLexicalSearcher {
    pub(crate) results: Vec<LexicalCandidate>,
    /// The sealed digest the handle claims to have proved; `None` claims
    /// the fixture's `manifest-digest-9`.
    pub(crate) manifest_digest: Option<String>,
}

impl LexicalSearcher for StubLexicalSearcher {
    fn resident_bytes_estimate(&self) -> u64 {
        0
    }

    /// The stub holds every source-repo metadata authority: it answers
    /// projections and gates without refusing.
    fn artifact_identity(&self) -> LexicalArtifactIdentityV1 {
        let mut identity = stub_artifact_identity(RepoMetadataAuthoritiesV1::ALL);
        if let Some(digest) = &self.manifest_digest {
            identity.manifest_digest = digest.clone();
        }
        identity
    }

    fn search_constrained(
        &self,
        _query: &quanta_index_contract::LqQuery,
        _constraints: &QueryConstraintSetV1,
        page: &LexicalPageSpec,
        _budget: &RequestBudgetV1,
    ) -> Result<LexicalSearchPageV1, CoreError> {
        Ok(LexicalSearchPageV1 {
            code_search_stats: None,
            candidates: ranked_page(&self.results, page),
            exact_total: None,
        })
    }

    fn project_file_owners(
        &self,
        candidates: &[LexicalCandidate],
    ) -> Result<Vec<quanta_index_contract::FileOwnerProjectionRow>, CoreError> {
        Ok(candidates
            .iter()
            .map(|candidate| quanta_index_contract::FileOwnerProjectionRow {
                candidate_id: candidate.candidate_id.clone(),
                source_repo_id: candidate.source_repo_id.clone(),
                repo_id: candidate.repo_id.clone(),
                revision_id: candidate.revision_id.clone(),
                manifest_generation: candidate.manifest_generation,
                repo_relative_path: candidate.repo_relative_path.clone(),
                owners: Vec::new(),
            })
            .collect())
    }

    fn search_symbols(
        &self,
        _query: &quanta_index_contract::LqQuery,
        _top_k: u32,
        _budget: &RequestBudgetV1,
    ) -> Result<Vec<SymbolCandidate>, CoreError> {
        Ok(self.results.iter().map(symbol_fixture_candidate).collect())
    }

    fn search_symbols_constrained(
        &self,
        query: &LqQuery,
        _constraints: &QueryConstraintSetV1,
        page: &LexicalPageSpec,
        _budget: &RequestBudgetV1,
    ) -> Result<quanta_index_core::SymbolSearchPageV1, CoreError> {
        symbol_fixture_page(&self.results, query, page)
    }

    fn search_symbols_all(
        &self,
        query: &LqQuery,
        _budget: &RequestBudgetV1,
    ) -> Result<Vec<SymbolCandidate>, CoreError> {
        symbol_fixture_all(&self.results, query)
    }

    fn search_all(
        &self,
        query: &quanta_index_contract::LqQuery,
        _budget: &RequestBudgetV1,
    ) -> Result<Vec<LexicalCandidate>, CoreError> {
        validate_fixture_exact_all_count(query)?;
        Ok(self.results.clone())
    }

    fn candidate_presence(&self, candidate_id: &str) -> Result<CandidatePresenceV1, CoreError> {
        Ok(
            if self
                .results
                .iter()
                .any(|candidate| candidate.candidate_id == candidate_id)
            {
                CandidatePresenceV1::Indexed
            } else {
                CandidatePresenceV1::NotIndexed
            },
        )
    }

    fn explain_candidate(
        &self,
        _query: &quanta_index_contract::LqQuery,
        _constraints: &QueryConstraintSetV1,
        candidate_id: &str,
        _budget: &RequestBudgetV1,
    ) -> Result<LexicalCandidateExplanationV1, CoreError> {
        // The double scores every stub result at its carried score under
        // a unit boost; the boost arithmetic is the real adapter's to
        // prove.
        Ok(self
            .results
            .iter()
            .find(|candidate| candidate.candidate_id == candidate_id)
            .map_or(LexicalCandidateExplanationV1::NotIndexed, |candidate| {
                LexicalCandidateExplanationV1::Matched(quanta_index_core::LexicalScoreTraceV1 {
                    engine: LexicalScoreEngineV1::Bm25,
                    code_search_components: None,
                    code_search_rank_study: None,
                    engine_score: candidate.score,
                    boost_factor: 1.0,
                    emitted_score: candidate.score,
                })
            }))
    }

    /// The stub has no filter store: it admits every candidate. Filter
    /// admission is proved through `RecordingLexicalSearcher`, whose state
    /// configures the admitted set.
    fn admitted_candidates(
        &self,
        _query: &quanta_index_contract::LqQuery,
        _constraints: &QueryConstraintSetV1,
        candidate_ids: &BTreeSet<String>,
        _budget: &RequestBudgetV1,
    ) -> Result<BTreeSet<String>, CoreError> {
        Ok(candidate_ids.clone())
    }
}

pub(crate) struct StubLexicalOpener {
    pub(crate) results: Vec<LexicalCandidate>,
}

impl LexicalIndexOpenPort for StubLexicalOpener {
    fn preflight_query_primitives(
        &self,
        plan: &quanta_index_core::ValidatedLexicalPlan,
        budget: &quanta_index_core::RequestBudgetV1,
    ) -> Result<(), CoreError> {
        quanta_index_lexical::planner::LexicalPlanner::validate_query_primitives(
            plan,
            &quanta_index_lexical::regex::RegexPolicy::defaults(),
            budget,
        )
    }

    fn open(
        &self,
        _repo: &RepoId,
        _revision: &RevisionId,
        _generation: ManifestGeneration,
        _budget: &quanta_index_core::RequestBudgetV1,
    ) -> Result<Box<dyn LexicalSearcher>, CoreError> {
        Ok(Box::new(StubLexicalSearcher {
            results: self.results.clone(),
            manifest_digest: None,
        }))
    }

    fn open_proven(
        &self,
        candidate: &GenerationSnapshot,
    ) -> Result<Box<dyn LexicalSearcher>, CoreError> {
        self.open(
            &candidate.repo_id,
            &candidate.revision_id,
            candidate.manifest_generation,
            &quanta_index_core::RequestBudgetV1::unbounded(),
        )
    }
}

#[derive(Default)]
pub(crate) struct RecordingLexicalState {
    /// The physical seal reported by the opened handle. The default matches
    /// the shared ready-ledger fixture; tests can inject a conflicting seal.
    pub(crate) manifest_digest: Option<String>,
    pub(crate) primitive_queries: Vec<LqQuery>,
    pub(crate) search_top_ks: Vec<u32>,
    /// The boundary every text page was asked to continue after.
    pub(crate) search_afters: Vec<Option<quanta_index_contract::LexicalCursor>>,
    pub(crate) symbol_top_ks: Vec<u32>,
    pub(crate) opened_pins: Vec<(RepoId, RevisionId, ManifestGeneration)>,
    pub(crate) searched_queries: Vec<LqQuery>,
    pub(crate) searched_constraints: Vec<QueryConstraintSetV1>,
    pub(crate) symbol_constraints: Vec<QueryConstraintSetV1>,
    /// When set, a text search cancels the budget it was handed and
    /// answers as a native collect that observed the cancellation
    /// would (W5 phase 2).
    pub(crate) cancel_inside_search: bool,
    /// The source-repo metadata authorities the opened handle reports;
    /// `None` reports every one of them.
    pub(crate) repo_metadata: Option<RepoMetadataAuthoritiesV1>,
    /// How many times the handle's identity was asked for.
    pub(crate) identity_reads: usize,
    /// The candidate ids the filter-only admission plan admits (QI-BB-018
    /// 보완 #3); `None` admits every candidate asked about.
    pub(crate) admitted_ids: Option<BTreeSet<String>>,
    /// Every admission evaluation asked of the handle: the filter-only
    /// plan and the candidate ids it was asked about, in call order.
    pub(crate) admission_calls: Vec<(LqQuery, BTreeSet<String>)>,
    /// Every presence lookup asked of the handle, in call order (W10-R1
    /// execution truth).
    pub(crate) presence_checks: Vec<String>,
    /// Every candidate-trace asked of the handle, in call order (W10-R1
    /// execution truth).
    pub(crate) explained_candidates: Vec<String>,
}

pub(crate) struct RecordingLexicalSearcher {
    pub(crate) state: Arc<Mutex<RecordingLexicalState>>,
    pub(crate) results: Vec<LexicalCandidate>,
}

impl LexicalSearcher for RecordingLexicalSearcher {
    fn resident_bytes_estimate(&self) -> u64 {
        0
    }

    fn artifact_identity(&self) -> LexicalArtifactIdentityV1 {
        // A poisoned double still reports its identity: the test that
        // poisoned it is the one that failed.
        let mut guard = match self.state.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        };
        guard.identity_reads = guard.identity_reads.saturating_add(1);
        let repo_metadata = guard
            .repo_metadata
            .unwrap_or(RepoMetadataAuthoritiesV1::ALL);
        let manifest_digest = guard
            .manifest_digest
            .clone()
            .unwrap_or_else(|| "manifest-digest-9".to_string());
        drop(guard);
        let mut identity = stub_artifact_identity(repo_metadata);
        identity.manifest_digest = manifest_digest;
        identity
    }

    fn search_constrained(
        &self,
        query: &quanta_index_contract::LqQuery,
        constraints: &QueryConstraintSetV1,
        page: &LexicalPageSpec,
        budget: &RequestBudgetV1,
    ) -> Result<LexicalSearchPageV1, CoreError> {
        let mut guard = self
            .state
            .lock()
            .map_err(|err| CoreError::Storage(format!("lexical state poisoned: {err}")))?;
        guard.search_top_ks.push(page.fetch);
        guard.search_afters.push(page.after.clone());
        guard.searched_queries.push(query.clone());
        guard.searched_constraints.push(constraints.clone());
        let cancel_inside_search = guard.cancel_inside_search;
        drop(guard);
        if cancel_inside_search {
            budget.cancel_handle().cancel();
            budget.checkpoint("stub:collect")?;
        }
        Ok(LexicalSearchPageV1 {
            code_search_stats: None,
            candidates: ranked_page(&self.results, page),
            exact_total: None,
        })
    }

    fn project_file_owners(
        &self,
        candidates: &[LexicalCandidate],
    ) -> Result<Vec<quanta_index_contract::FileOwnerProjectionRow>, CoreError> {
        Ok(candidates
            .iter()
            .map(|candidate| quanta_index_contract::FileOwnerProjectionRow {
                candidate_id: candidate.candidate_id.clone(),
                repo_id: candidate.repo_id.clone(),
                source_repo_id: candidate.source_repo_id.clone(),
                revision_id: candidate.revision_id.clone(),
                manifest_generation: candidate.manifest_generation,
                repo_relative_path: candidate.repo_relative_path.clone(),
                owners: Vec::new(),
            })
            .collect())
    }

    fn search_symbols(
        &self,
        _query: &quanta_index_contract::LqQuery,
        top_k: u32,
        _budget: &RequestBudgetV1,
    ) -> Result<Vec<SymbolCandidate>, CoreError> {
        self.state
            .lock()
            .map_err(|err| CoreError::Storage(format!("lexical state poisoned: {err}")))?
            .symbol_top_ks
            .push(top_k);
        Ok(self.results.iter().map(symbol_fixture_candidate).collect())
    }

    fn search_symbols_constrained(
        &self,
        query: &LqQuery,
        constraints: &QueryConstraintSetV1,
        page: &LexicalPageSpec,
        _budget: &RequestBudgetV1,
    ) -> Result<quanta_index_core::SymbolSearchPageV1, CoreError> {
        let mut guard = self
            .state
            .lock()
            .map_err(|err| CoreError::Storage(format!("lexical state poisoned: {err}")))?;
        guard.symbol_top_ks.push(page.fetch);
        guard.symbol_constraints.push(constraints.clone());
        drop(guard);
        symbol_fixture_page(&self.results, query, page)
    }

    fn search_symbols_all(
        &self,
        query: &LqQuery,
        _budget: &RequestBudgetV1,
    ) -> Result<Vec<SymbolCandidate>, CoreError> {
        // Keep the fixture's existing all-port invocation marker. The complete
        // fixture set below is never obtained from a capped page request.
        self.state
            .lock()
            .map_err(|error| CoreError::Storage(format!("lexical state poisoned: {error}")))?
            .symbol_top_ks
            .push(u32::MAX);
        symbol_fixture_all(&self.results, query)
    }

    fn search_all(
        &self,
        query: &quanta_index_contract::LqQuery,
        _budget: &RequestBudgetV1,
    ) -> Result<Vec<LexicalCandidate>, CoreError> {
        validate_fixture_exact_all_count(query)?;
        Ok(self.results.clone())
    }

    fn candidate_presence(&self, candidate_id: &str) -> Result<CandidatePresenceV1, CoreError> {
        self.state
            .lock()
            .map_err(|err| CoreError::Storage(format!("lexical state poisoned: {err}")))?
            .presence_checks
            .push(candidate_id.to_string());
        Ok(
            if self
                .results
                .iter()
                .any(|candidate| candidate.candidate_id == candidate_id)
            {
                CandidatePresenceV1::Indexed
            } else {
                CandidatePresenceV1::NotIndexed
            },
        )
    }

    fn explain_candidate(
        &self,
        _query: &quanta_index_contract::LqQuery,
        _constraints: &QueryConstraintSetV1,
        candidate_id: &str,
        _budget: &RequestBudgetV1,
    ) -> Result<LexicalCandidateExplanationV1, CoreError> {
        self.state
            .lock()
            .map_err(|err| CoreError::Storage(format!("lexical state poisoned: {err}")))?
            .explained_candidates
            .push(candidate_id.to_string());
        // The double scores every stub result at its carried score under
        // a unit boost; the boost arithmetic is the real adapter's to
        // prove.
        Ok(self
            .results
            .iter()
            .find(|candidate| candidate.candidate_id == candidate_id)
            .map_or(LexicalCandidateExplanationV1::NotIndexed, |candidate| {
                LexicalCandidateExplanationV1::Matched(quanta_index_core::LexicalScoreTraceV1 {
                    engine: LexicalScoreEngineV1::Bm25,
                    code_search_components: None,
                    code_search_rank_study: None,
                    engine_score: candidate.score,
                    boost_factor: 1.0,
                    emitted_score: candidate.score,
                })
            }))
    }

    fn admitted_candidates(
        &self,
        query: &quanta_index_contract::LqQuery,
        _constraints: &QueryConstraintSetV1,
        candidate_ids: &BTreeSet<String>,
        _budget: &RequestBudgetV1,
    ) -> Result<BTreeSet<String>, CoreError> {
        let mut guard = self
            .state
            .lock()
            .map_err(|err| CoreError::Storage(format!("lexical state poisoned: {err}")))?;
        guard
            .admission_calls
            .push((query.clone(), candidate_ids.clone()));
        let admitted = guard.admitted_ids.as_ref().map_or_else(
            || candidate_ids.clone(),
            |admitted| candidate_ids.intersection(admitted).cloned().collect(),
        );
        drop(guard);
        Ok(admitted)
    }
}

pub(crate) struct RecordingLexicalOpener {
    pub(crate) state: Arc<Mutex<RecordingLexicalState>>,
    pub(crate) results: Vec<LexicalCandidate>,
}

impl LexicalIndexOpenPort for RecordingLexicalOpener {
    fn preflight_query_primitives(
        &self,
        plan: &quanta_index_core::ValidatedLexicalPlan,
        budget: &quanta_index_core::RequestBudgetV1,
    ) -> Result<(), CoreError> {
        self.state
            .lock()
            .map_err(|err| CoreError::Storage(format!("lexical state poisoned: {err}")))?
            .primitive_queries
            .push(plan.query().clone());
        quanta_index_lexical::planner::LexicalPlanner::validate_query_primitives(
            plan,
            &quanta_index_lexical::regex::RegexPolicy::defaults(),
            budget,
        )
    }

    fn open(
        &self,
        repo: &RepoId,
        revision: &RevisionId,
        generation: ManifestGeneration,
        _budget: &quanta_index_core::RequestBudgetV1,
    ) -> Result<Box<dyn LexicalSearcher>, CoreError> {
        self.state
            .lock()
            .map_err(|err| CoreError::Storage(format!("lexical state poisoned: {err}")))?
            .opened_pins
            .push((repo.clone(), revision.clone(), generation));
        Ok(Box::new(RecordingLexicalSearcher {
            state: Arc::clone(&self.state),
            results: self.results.clone(),
        }))
    }

    fn open_proven(
        &self,
        candidate: &GenerationSnapshot,
    ) -> Result<Box<dyn LexicalSearcher>, CoreError> {
        self.open(
            &candidate.repo_id,
            &candidate.revision_id,
            candidate.manifest_generation,
            &quanta_index_core::RequestBudgetV1::unbounded(),
        )
    }
}

pub(crate) fn recording_lexical_candidate(candidate_id: &str) -> LexicalCandidate {
    LexicalCandidate {
        candidate_id: candidate_id.to_string(),
        source_repo_id: RepoId::new("repo-map-ipc").expect("static fixture source ID is canonical"),
        repo_id: RepoId::new("repo-map-ipc").expect("static fixture ID satisfies canonical policy"),
        revision_id: RevisionId::new("rev-map-ipc")
            .expect("static fixture ID satisfies canonical policy"),
        manifest_generation: ManifestGeneration::new(9),
        repo_relative_path: RepoRelativePath::new("src/a.rs"),
        start_line: 1,
        end_line: 1,
        score: 1.0,
        snippet: candidate_id.to_string(),
        snippet_hit_offset: None,
        highlights: Vec::new(),
        source: None,
        preview: None,
    }
}

/// What a real adapter returns for `page` over `results`: the rows after
/// the boundary, in page order, at most the fetch.
pub(crate) fn ranked_page(
    results: &[LexicalCandidate],
    page: &LexicalPageSpec,
) -> Vec<LexicalCandidate> {
    let mut rows: Vec<LexicalCandidate> = results
        .iter()
        .filter(|row| {
            page.after
                .as_ref()
                .is_none_or(|cursor| cursor.admits(&row.order_key()))
        })
        .cloned()
        .collect();
    rows.sort_by(|left, right| left.order_key().order(&right.order_key()));
    rows.truncate(usize::try_from(page.fetch).map_or(usize::MAX, |fetch| fetch));
    rows
}

/// A complete, independently supplied fixture universe; count before clipping.
fn symbol_fixture_page(
    results: &[LexicalCandidate],
    query: &LqQuery,
    page: &LexicalPageSpec,
) -> Result<quanta_index_core::SymbolSearchPageV1, CoreError> {
    let mut rows: Vec<_> = results
        .iter()
        .map(symbol_fixture_candidate)
        .filter(|row| {
            page.after
                .as_ref()
                .is_none_or(|cursor| cursor.admits(&row.order_key()))
        })
        .collect();
    rows.sort_by(|left, right| left.order_key().order(&right.order_key()));
    let exact_total = if query.options.count.is_some() {
        Some(u64::try_from(rows.len()).map_err(|err| CoreError::InvalidContract(err.to_string()))?)
    } else {
        None
    };
    let limit = match query.options.count {
        Some(quanta_index_contract::LqCountBound::Bounded(n)) => page.fetch.min(n),
        Some(quanta_index_contract::LqCountBound::All) | None => page.fetch,
    };
    rows.truncate(
        usize::try_from(limit).map_err(|err| CoreError::InvalidContract(err.to_string()))?,
    );
    Ok(quanta_index_core::SymbolSearchPageV1 {
        candidates: rows,
        exact_total,
    })
}

fn symbol_fixture_candidate(candidate: &LexicalCandidate) -> SymbolCandidate {
    let mut row = symbol_candidate(candidate.candidate_id.as_str(), candidate.score);
    row.repo_id = candidate.repo_id.clone();
    row.source_repo_id = candidate.source_repo_id.clone();
    row.revision_id = candidate.revision_id.clone();
    row.manifest_generation = candidate.manifest_generation;
    row.repo_relative_path = candidate.repo_relative_path.clone();
    row.start_line = candidate.start_line;
    row.end_line = candidate.end_line;
    row.snippet = candidate.snippet.clone();
    row.source = candidate.source.clone();
    row.preview = candidate.preview.clone();
    row
}

fn symbol_fixture_all(
    results: &[LexicalCandidate],
    query: &LqQuery,
) -> Result<Vec<SymbolCandidate>, CoreError> {
    validate_fixture_exact_all_count(query)?;
    let mut rows: Vec<_> = results.iter().map(symbol_fixture_candidate).collect();
    rows.sort_by(|left, right| left.order_key().order(&right.order_key()));
    Ok(rows)
}

fn validate_fixture_exact_all_count(query: &LqQuery) -> Result<(), CoreError> {
    if matches!(
        query.options.count,
        Some(quanta_index_contract::LqCountBound::Bounded(_))
    ) {
        return Err(CoreError::Typed {
            code: quanta_index_contract::SearchPlaneErrorCodeV2::LexFilterInvalidCount,
            message: "exact-all fixture cannot honor a bounded count".into(),
        });
    }
    Ok(())
}
