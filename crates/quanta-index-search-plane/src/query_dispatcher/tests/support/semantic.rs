//! Semantic opener / searcher / embedder test doubles.

use std::sync::{Arc, Mutex};

use quanta_index_contract::{
    ClusterMembershipBatchReadRequestV1, ClusterMembershipBatchReadResponseV1, GenerationPin,
    GenerationSnapshot, LexicalCandidate, ManifestGeneration, QueryConstraintSetV1, RepoId,
    RevisionId, SemanticCorpusKindV1, SemanticQueryRequest,
};
use quanta_index_core::{
    CoreError, DenseIndexV1, DenseLaneAttestationV1, DenseLaneContractV1, RequestBudgetV1,
    SemanticIndexOpenPort, SemanticSearchHitV1, SemanticSearcher,
};

use crate::query_dispatcher::tests::support::common::{candidate, ready_pin};
use crate::{HashingQueryTextEmbedder, QueryTextEmbedderPort};

pub(crate) fn exact_dense_lane() -> DenseLaneContractV1 {
    DenseLaneContractV1 {
        index: DenseIndexV1::Exact,
        attestation: DenseLaneAttestationV1::Sealed,
    }
}

pub(crate) struct RejectSemanticOpener;

impl SemanticIndexOpenPort for RejectSemanticOpener {
    fn open(
        &self,
        _repo: &RepoId,
        _revision: &RevisionId,
        _generation: ManifestGeneration,
    ) -> Result<Box<dyn SemanticSearcher>, CoreError> {
        Err(CoreError::NotImplemented(
            "repo-map dispatch should not open semantic index".to_string(),
        ))
    }

    fn open_proven(
        &self,
        candidate: &GenerationSnapshot,
    ) -> Result<Box<dyn SemanticSearcher>, CoreError> {
        self.open(
            &candidate.repo_id,
            &candidate.revision_id,
            candidate.manifest_generation,
        )
    }
}

pub(crate) fn cluster_membership_batch_request_v1() -> ClusterMembershipBatchReadRequestV1 {
    ClusterMembershipBatchReadRequestV1 {
        generation: ready_pin(),
        items: vec![
            quanta_index_contract::ClusterMembershipBatchReadItemV1 {
                cluster_record_id: "cluster-card:auth".to_string(),
                expected_authority_digest: "authority:auth".to_string(),
                limit: 2,
            },
            quanta_index_contract::ClusterMembershipBatchReadItemV1 {
                cluster_record_id: "cluster-card:billing".to_string(),
                expected_authority_digest: "authority:billing".to_string(),
                limit: 2,
            },
        ],
    }
}

pub(crate) fn available_cluster_membership_batch_response_v1(
    request: &ClusterMembershipBatchReadRequestV1,
) -> ClusterMembershipBatchReadResponseV1 {
    ClusterMembershipBatchReadResponseV1 {
        outcomes: request
            .items
            .iter()
            .map(|item| {
                quanta_index_contract::ClusterMembershipReadOutcomeV1::Available(
                    quanta_index_contract::ClusterMembershipSnapshotV1 {
                        cluster_record_id: item.cluster_record_id.clone(),
                        generation: request.generation.clone(),
                        authority_digest: item.expected_authority_digest.clone(),
                        members: vec![quanta_index_contract::SymbolId::new(format!(
                            "symbol:{}",
                            item.cluster_record_id
                        ))],
                        completeness:
                            quanta_index_contract::ClusterMembershipCompletenessV1::Complete,
                    },
                )
            })
            .collect(),
    }
}

#[derive(Default)]
pub(crate) struct RecordingSemanticState {
    /// What `search_constrained` answers, cut to the `top_k` asked for;
    /// `None` answers the one inline `semantic-inline` hit.
    pub(crate) constrained_search_results: Option<Vec<LexicalCandidate>>,
    pub(crate) search_vectors: Vec<Vec<f32>>,
    /// The `top_k` of every `search_constrained` call, in call order: the
    /// dense lane's fetch sizes (QI-BB-018 보완 #3 refill).
    pub(crate) search_top_ks: Vec<u32>,
    pub(crate) search_hit_vectors: Vec<Vec<f32>>,
    pub(crate) corpus_searches: Vec<(SemanticCorpusKindV1, u32)>,
    pub(crate) scoped_vectors: Vec<Vec<f32>>,
    pub(crate) search_constraints: Vec<QueryConstraintSetV1>,
    pub(crate) search_hit_constraints: Vec<QueryConstraintSetV1>,
    pub(crate) corpus_constraints: Vec<QueryConstraintSetV1>,
    pub(crate) scoped_constraints: Vec<QueryConstraintSetV1>,
    pub(crate) cluster_membership_opened_pins: Vec<(RepoId, RevisionId, ManifestGeneration)>,
    pub(crate) cluster_membership_requests: Vec<ClusterMembershipBatchReadRequestV1>,
    pub(crate) cluster_membership_response: Option<ClusterMembershipBatchReadResponseV1>,
    /// When set, every dense search cancels the budget it was handed and
    /// answers as a lane that observed the cancellation inside would (W5
    /// phase 3).
    pub(crate) cancel_inside_search: bool,
    /// The sealed digest the opened handle claims to have proved; `None`
    /// claims the fixture's `manifest-digest-9`.
    pub(crate) manifest_digest: Option<String>,
}

pub(crate) struct RecordingSemanticSearcher {
    pub(crate) state: Arc<Mutex<RecordingSemanticState>>,
    /// Read once at open from the state's `manifest_digest`, so the
    /// borrow the trait hands out needs no lock.
    pub(crate) manifest_digest: String,
}

impl RecordingSemanticSearcher {
    /// Whether the recorded state asks searches to cancel their budget.
    fn cancels_inside_search(&self) -> Result<bool, CoreError> {
        Ok(self
            .state
            .lock()
            .map_err(|err| CoreError::Storage(format!("semantic state poisoned: {err}")))?
            .cancel_inside_search)
    }

    /// The stub's own observation of the budget: cancel it and report the
    /// interruption at the stub's dense checkpoint, when asked to.
    fn observe_budget(&self, budget: &RequestBudgetV1) -> Result<(), CoreError> {
        if self.cancels_inside_search()? {
            budget.cancel_handle().cancel();
            budget.checkpoint("stub:dense")?;
        }
        Ok(())
    }
}

impl SemanticSearcher for RecordingSemanticSearcher {
    fn resident_bytes_estimate(&self) -> u64 {
        0
    }

    fn cluster_membership_batch_read(
        &self,
        request: &ClusterMembershipBatchReadRequestV1,
    ) -> Result<ClusterMembershipBatchReadResponseV1, CoreError> {
        let recorded_response = {
            let mut state = self
                .state
                .lock()
                .map_err(|err| CoreError::Storage(format!("semantic state poisoned: {err}")))?;
            state.cluster_membership_requests.push(request.clone());
            state.cluster_membership_response.clone()
        };
        if let Some(response) = recorded_response {
            return Ok(response);
        }
        Ok(ClusterMembershipBatchReadResponseV1 {
            outcomes: request
                .items
                .iter()
                .map(|item| {
                    quanta_index_contract::ClusterMembershipReadOutcomeV1::Rejected(
                        quanta_index_contract::ClusterMembershipReadRejectionV1 {
                            cluster_record_id: item.cluster_record_id.clone(),
                            generation: request.generation.clone(),
                            expected_authority_digest: item
                                .expected_authority_digest
                                .clone(),
                            failure: quanta_index_contract::ClusterMembershipReadFailureV1::CurrentGenerationMissing,
                        },
                    )
                })
                .collect(),
        })
    }

    fn search(
        &self,
        query_vector: &[f32],
        _top_k: u32,
        budget: &RequestBudgetV1,
    ) -> Result<Vec<LexicalCandidate>, CoreError> {
        self.state
            .lock()
            .map_err(|err| CoreError::Storage(format!("semantic state poisoned: {err}")))?
            .search_vectors
            .push(query_vector.to_vec());
        self.observe_budget(budget)?;
        Ok(vec![candidate("semantic-inline", 1.0)])
    }

    fn search_constrained(
        &self,
        query_vector: &[f32],
        constraints: &QueryConstraintSetV1,
        top_k: u32,
        budget: &RequestBudgetV1,
    ) -> Result<Vec<LexicalCandidate>, CoreError> {
        let configured = {
            let mut state = self
                .state
                .lock()
                .map_err(|err| CoreError::Storage(format!("semantic state poisoned: {err}")))?;
            state.search_vectors.push(query_vector.to_vec());
            state.search_constraints.push(constraints.clone());
            state.search_top_ks.push(top_k);
            state.constrained_search_results.clone()
        };
        self.observe_budget(budget)?;
        if let Some(mut rows) = configured {
            // A ranked engine answers at most `top_k` rows.
            rows.truncate(usize::try_from(top_k).map_err(|err| {
                CoreError::InvalidContract(format!("stub top_k overflow: {err}"))
            })?);
            return Ok(rows);
        }
        Ok(vec![candidate("semantic-inline", 1.0)])
    }

    fn search_hits(
        &self,
        query_vector: &[f32],
        _top_k: u32,
        budget: &RequestBudgetV1,
    ) -> Result<Vec<SemanticSearchHitV1>, CoreError> {
        self.state
            .lock()
            .map_err(|err| CoreError::Storage(format!("semantic state poisoned: {err}")))?
            .search_hit_vectors
            .push(query_vector.to_vec());
        self.observe_budget(budget)?;
        Ok(vec![SemanticSearchHitV1 {
            candidate: candidate("semantic-inline", 1.0),
            record_id: "semantic-inline-record".to_string(),
            owner_id: "semantic-inline-owner".to_string(),
            owner_kind: quanta_index_contract::OwnerDocKind::Chunk,
            corpus_kind: None,
            authority_digest: "authority:semantic-inline".to_string(),
        }])
    }

    fn search_hits_constrained(
        &self,
        query_vector: &[f32],
        constraints: &QueryConstraintSetV1,
        _top_k: u32,
        budget: &RequestBudgetV1,
    ) -> Result<Vec<SemanticSearchHitV1>, CoreError> {
        {
            let mut state = self
                .state
                .lock()
                .map_err(|err| CoreError::Storage(format!("semantic state poisoned: {err}")))?;
            state.search_hit_vectors.push(query_vector.to_vec());
            state.search_hit_constraints.push(constraints.clone());
        }
        self.observe_budget(budget)?;
        Ok(vec![SemanticSearchHitV1 {
            candidate: candidate("semantic-inline", 1.0),
            record_id: "semantic-inline-record".to_string(),
            owner_id: "semantic-inline-owner".to_string(),
            owner_kind: quanta_index_contract::OwnerDocKind::Chunk,
            corpus_kind: None,
            authority_digest: "authority:semantic-inline".to_string(),
        }])
    }

    fn search_hits_for_corpus(
        &self,
        query_vector: &[f32],
        corpus_kind: SemanticCorpusKindV1,
        top_k: u32,
        budget: &RequestBudgetV1,
    ) -> Result<Vec<SemanticSearchHitV1>, CoreError> {
        let mut state = self
            .state
            .lock()
            .map_err(|err| CoreError::Storage(format!("semantic state poisoned: {err}")))?;
        state.search_hit_vectors.push(query_vector.to_vec());
        state.corpus_searches.push((corpus_kind, top_k));
        drop(state);
        self.observe_budget(budget)?;
        if corpus_kind == SemanticCorpusKindV1::RepositorySummary {
            return Ok(Vec::new());
        }
        Ok(vec![SemanticSearchHitV1 {
            candidate: candidate(corpus_kind.as_code_str(), 1.0),
            record_id: format!("record:{}", corpus_kind.as_code_str()),
            owner_id: format!("owner:{}", corpus_kind.as_code_str()),
            owner_kind: quanta_index_contract::OwnerDocKind::Symbol,
            corpus_kind: Some(corpus_kind),
            authority_digest: format!("authority:{}", corpus_kind.as_code_str()),
        }])
    }

    fn search_hits_for_corpus_constrained(
        &self,
        query_vector: &[f32],
        corpus_kind: SemanticCorpusKindV1,
        constraints: &QueryConstraintSetV1,
        top_k: u32,
        budget: &RequestBudgetV1,
    ) -> Result<Vec<SemanticSearchHitV1>, CoreError> {
        let mut state = self
            .state
            .lock()
            .map_err(|err| CoreError::Storage(format!("semantic state poisoned: {err}")))?;
        state.search_hit_vectors.push(query_vector.to_vec());
        state.corpus_searches.push((corpus_kind, top_k));
        state.corpus_constraints.push(constraints.clone());
        drop(state);
        self.observe_budget(budget)?;
        if corpus_kind == SemanticCorpusKindV1::RepositorySummary {
            return Ok(Vec::new());
        }
        Ok(vec![SemanticSearchHitV1 {
            candidate: candidate(corpus_kind.as_code_str(), 1.0),
            record_id: format!("record:{}", corpus_kind.as_code_str()),
            owner_id: format!("owner:{}", corpus_kind.as_code_str()),
            owner_kind: quanta_index_contract::OwnerDocKind::Symbol,
            corpus_kind: Some(corpus_kind),
            authority_digest: format!("authority:{}", corpus_kind.as_code_str()),
        }])
    }

    fn search_scoped(
        &self,
        query_vector: &[f32],
        _allowed_ids: &std::collections::BTreeSet<String>,
        _top_k: u32,
        budget: &RequestBudgetV1,
    ) -> Result<Vec<LexicalCandidate>, CoreError> {
        self.state
            .lock()
            .map_err(|err| CoreError::Storage(format!("semantic state poisoned: {err}")))?
            .scoped_vectors
            .push(query_vector.to_vec());
        self.observe_budget(budget)?;
        Ok(vec![candidate("semantic-scoped", 1.0)])
    }

    fn search_scoped_constrained(
        &self,
        query_vector: &[f32],
        _allowed_ids: &std::collections::BTreeSet<String>,
        constraints: &QueryConstraintSetV1,
        _top_k: u32,
        budget: &RequestBudgetV1,
    ) -> Result<Vec<LexicalCandidate>, CoreError> {
        {
            let mut state = self
                .state
                .lock()
                .map_err(|err| CoreError::Storage(format!("semantic state poisoned: {err}")))?;
            state.scoped_vectors.push(query_vector.to_vec());
            state.scoped_constraints.push(constraints.clone());
        }
        self.observe_budget(budget)?;
        Ok(vec![candidate("semantic-scoped", 1.0)])
    }

    fn index_model_id(&self) -> &str {
        // Match the HashingQueryTextEmbedder these tests query with, so the
        // model-identity gate passes on the matching path (the mismatch path
        // is covered by ensure_query_model_matches_index_v1's unit test).
        crate::SEARCH_OWNED_SEMANTIC_MODEL_ID
    }

    fn index_model_revision(&self) -> Option<&str> {
        Some(crate::query_embedder::SEARCH_OWNED_SEMANTIC_MODEL_REVISION)
    }

    fn manifest_digest(&self) -> &str {
        &self.manifest_digest
    }

    fn dense_lane(&self) -> DenseLaneContractV1 {
        DenseLaneContractV1 {
            index: DenseIndexV1::Exact,
            attestation: DenseLaneAttestationV1::Sealed,
        }
    }
}

pub(crate) struct RecordingSemanticOpener {
    pub(crate) state: Arc<Mutex<RecordingSemanticState>>,
}

impl SemanticIndexOpenPort for RecordingSemanticOpener {
    fn open(
        &self,
        repo: &RepoId,
        revision: &RevisionId,
        generation: ManifestGeneration,
    ) -> Result<Box<dyn SemanticSearcher>, CoreError> {
        let manifest_digest = {
            let mut state = self
                .state
                .lock()
                .map_err(|err| CoreError::Storage(format!("semantic state poisoned: {err}")))?;
            state
                .cluster_membership_opened_pins
                .push((repo.clone(), revision.clone(), generation));
            state
                .manifest_digest
                .clone()
                .unwrap_or_else(|| "manifest-digest-9".to_string())
        };
        Ok(Box::new(RecordingSemanticSearcher {
            state: Arc::clone(&self.state),
            manifest_digest,
        }))
    }

    fn open_proven(
        &self,
        candidate: &GenerationSnapshot,
    ) -> Result<Box<dyn SemanticSearcher>, CoreError> {
        self.open(
            &candidate.repo_id,
            &candidate.revision_id,
            candidate.manifest_generation,
        )
    }
}

/// Embeds a real vector but advertises a configurable model identity, so a
/// query-time model drift can be exercised at the dispatcher boundary.
pub(crate) struct FixedModelQueryEmbedder {
    pub(crate) model_id: &'static str,
    pub(crate) model_revision: &'static str,
    pub(crate) dimension: usize,
}

impl QueryTextEmbedderPort for FixedModelQueryEmbedder {
    fn embed_query(
        &self,
        query_text: &str,
        budget: &quanta_index_core::RequestBudgetV1,
    ) -> Result<Vec<f32>, quanta_index_core::CoreError> {
        HashingQueryTextEmbedder::new(self.dimension).embed_query(query_text, budget)
    }
    fn model_id(&self) -> &'static str {
        self.model_id
    }
    fn model_revision(&self) -> &'static str {
        self.model_revision
    }
}

/// Always fails `embed_query` (provider down), to prove the model gate runs
/// AFTER embed and does not mask the provider-unavailable rail.
pub(crate) struct UnavailableTestQueryEmbedder;

impl QueryTextEmbedderPort for UnavailableTestQueryEmbedder {
    fn embed_query(
        &self,
        _query_text: &str,
        _budget: &quanta_index_core::RequestBudgetV1,
    ) -> Result<Vec<f32>, quanta_index_core::CoreError> {
        Err(quanta_index_core::CoreError::Typed {
            code: quanta_index_contract::lex::LexicalErrorCode::SemProviderUnavailable
                .as_code_str()
                .to_string(),
            message: "test embedder unavailable".to_string(),
        })
    }
    fn model_id(&self) -> &'static str {
        "provider-unavailable"
    }
    fn model_revision(&self) -> &'static str {
        "unavailable"
    }
}

pub(crate) fn semantic_focus_request() -> SemanticQueryRequest {
    SemanticQueryRequest {
        query_text: "focus alpha".to_string(),
        constraints: quanta_index_contract::QueryConstraintSetV1::unconstrained(),
        generation: Some(GenerationPin::new(
            RepoId::new("repo-map-ipc"),
            RevisionId::new("rev-map-ipc"),
            ManifestGeneration::new(9),
        )),
        generation_selector: None,
        lexical_scope: None,
        top_k: 3,
    }
}
