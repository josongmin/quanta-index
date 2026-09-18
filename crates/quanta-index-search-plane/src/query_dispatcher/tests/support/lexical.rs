//! Lexical opener / searcher test doubles.

use std::collections::BTreeSet;
use std::sync::{Arc, Mutex};

use quanta_index_contract::{
    CandidatePresenceV1, GenerationSnapshot, LexicalCandidate, LqQuery, ManifestGeneration,
    QueryConstraintSetV1, RepoId, RepoRelativePath, RevisionId, SymbolCandidate,
};
use quanta_index_core::{
    CoreError, LexicalArtifactIdentityV1, LexicalCandidateExplanationV1, LexicalIndexOpenPort,
    LexicalScoreEngineV1, LexicalSearchPageV1, LexicalSearcher, RepoMetadataAuthoritiesV1,
    RequestBudgetV1, TextNormalizerVersionV1,
};

use crate::query_dispatcher::tests::support::common::symbol_candidate;

/// The identity a test double reports: a fixed digest and normalizer,
/// and the source-repo metadata authorities the double claims to hold.
pub(crate) fn stub_artifact_identity(
    repo_metadata: RepoMetadataAuthoritiesV1,
) -> LexicalArtifactIdentityV1 {
    LexicalArtifactIdentityV1 {
        manifest_digest: "stub-lexical-digest".to_string(),
        normalizer: TextNormalizerVersionV1 { major: 2, minor: 0 },
        repo_metadata,
    }
}

pub(crate) struct RejectLexicalOpener;

impl LexicalIndexOpenPort for RejectLexicalOpener {
    fn open(
        &self,
        _repo: &RepoId,
        _revision: &RevisionId,
        _generation: ManifestGeneration,
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
        )
    }
}

#[derive(Default)]
pub(crate) struct StubLexicalSearcher {
    pub(crate) results: Vec<LexicalCandidate>,
    /// The sealed digest the handle claims to have proved; `None` claims
    /// the fixture's `stub-lexical-digest`.
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
        _top_k: u32,
        _budget: &RequestBudgetV1,
    ) -> Result<LexicalSearchPageV1, CoreError> {
        Ok(LexicalSearchPageV1 {
            candidates: self.results.clone(),
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
        Ok(self
            .results
            .iter()
            .map(|candidate| symbol_candidate(candidate.candidate_id.as_str(), candidate.score))
            .collect())
    }

    fn search_all(
        &self,
        _query: &quanta_index_contract::LqQuery,
        _budget: &RequestBudgetV1,
    ) -> Result<Vec<LexicalCandidate>, CoreError> {
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
    fn open(
        &self,
        _repo: &RepoId,
        _revision: &RevisionId,
        _generation: ManifestGeneration,
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
        )
    }
}

#[derive(Default)]
pub(crate) struct RecordingLexicalState {
    pub(crate) search_top_ks: Vec<u32>,
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
        drop(guard);
        stub_artifact_identity(repo_metadata)
    }

    fn search_constrained(
        &self,
        query: &quanta_index_contract::LqQuery,
        constraints: &QueryConstraintSetV1,
        top_k: u32,
        budget: &RequestBudgetV1,
    ) -> Result<LexicalSearchPageV1, CoreError> {
        let mut guard = self
            .state
            .lock()
            .map_err(|err| CoreError::Storage(format!("lexical state poisoned: {err}")))?;
        guard.search_top_ks.push(top_k);
        guard.searched_queries.push(query.clone());
        guard.searched_constraints.push(constraints.clone());
        let cancel_inside_search = guard.cancel_inside_search;
        drop(guard);
        if cancel_inside_search {
            budget.cancel_handle().cancel();
            budget.checkpoint("stub:collect")?;
        }
        Ok(LexicalSearchPageV1 {
            candidates: self.results.clone(),
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
        Ok(self
            .results
            .iter()
            .map(|candidate| symbol_candidate(candidate.candidate_id.as_str(), candidate.score))
            .collect())
    }

    fn search_symbols_constrained(
        &self,
        _query: &quanta_index_contract::LqQuery,
        constraints: &QueryConstraintSetV1,
        top_k: u32,
        _budget: &RequestBudgetV1,
    ) -> Result<Vec<SymbolCandidate>, CoreError> {
        let mut guard = self
            .state
            .lock()
            .map_err(|err| CoreError::Storage(format!("lexical state poisoned: {err}")))?;
        guard.symbol_top_ks.push(top_k);
        guard.symbol_constraints.push(constraints.clone());
        drop(guard);
        Ok(self
            .results
            .iter()
            .map(|candidate| symbol_candidate(candidate.candidate_id.as_str(), candidate.score))
            .collect())
    }

    fn search_all(
        &self,
        _query: &quanta_index_contract::LqQuery,
        _budget: &RequestBudgetV1,
    ) -> Result<Vec<LexicalCandidate>, CoreError> {
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
    fn open(
        &self,
        repo: &RepoId,
        revision: &RevisionId,
        generation: ManifestGeneration,
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
        )
    }
}

pub(crate) fn recording_lexical_candidate(candidate_id: &str) -> LexicalCandidate {
    LexicalCandidate {
        candidate_id: candidate_id.to_string(),
        repo_id: RepoId::new("repo-map-ipc"),
        revision_id: RevisionId::new("rev-map-ipc"),
        manifest_generation: ManifestGeneration::new(9),
        repo_relative_path: RepoRelativePath::new("src/a.rs"),
        start_line: 1,
        end_line: 1,
        score: 1.0,
        snippet: candidate_id.to_string(),
        snippet_hit_offset: None,
        highlights: Vec::new(),
    }
}
