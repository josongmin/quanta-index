use quanta_index_contract::{
    ClusterMembershipBatchReadRequestV1, ClusterMembershipBatchReadResponseV1,
    ClusterMembershipReadOutcomeV1, ClusterMembershipReadRequestV1, ExplainCandidateV1,
    GenerationPin, GenerationSelector, HybridCandidateV1, HybridQueryRequest, HybridQueryResponse,
    HybridSeedQueryRequest, HybridSeedQueryResponse, LexicalCandidate, RepoId, RevisionId,
    SearchPlaneExplainQueryRequest, SearchPlaneExplainQueryResponse, SemanticCorpusKindV1,
    TextQueryRequest,
};

use crate::{QuantaIndex, SdkError, text_query_builder::VectorQueryBuilderState};

pub struct SearchNamespace<'a> {
    client: &'a QuantaIndex,
}

impl<'a> SearchNamespace<'a> {
    pub(super) const fn new(client: &'a QuantaIndex) -> Self {
        Self { client }
    }

    /// Hybrid search (QI-BB-018): two independent, bounded lanes fused by
    /// reciprocal rank fusion.
    ///
    /// The lexical lane runs the text query over the lexical index; the
    /// dense lane embeds `semantic_text` and ranks the whole generation
    /// under the same constraints. Their union is fused, so a document the
    /// lexical lane never matched can enter the top-k on dense relevance
    /// alone; every row carries its RRF score and per-lane provenance. This
    /// is not the semantic route's `lexical_scope`, which confines the dense
    /// lane to the lexical matches (a lexical-scoped rerank).
    #[must_use]
    pub fn hybrid(&self) -> HybridQueryBuilder<'a> {
        HybridQueryBuilder::new(self.client)
    }

    /// Dispatch a fully assembled hybrid request (QI-BB-018).
    pub fn hybrid_request(
        &self,
        request: HybridQueryRequest,
    ) -> Result<HybridQueryResponse, SdkError> {
        dispatch_hybrid_query_request_v1(self.client, request)
    }

    #[must_use]
    pub fn hybrid_seed(&self) -> HybridSeedQueryBuilder<'a> {
        HybridSeedQueryBuilder::new(self.client)
    }

    pub fn hybrid_seed_request(
        &self,
        request: HybridSeedQueryRequest,
    ) -> Result<HybridSeedQueryResponse, SdkError> {
        dispatch_hybrid_seed_query_request_v1(self.client, request)
    }

    /// Reads the structured members of one generation-pinned `ClusterCard`.
    ///
    /// The SDK validates both the request policy and the complete response
    /// authority tuple before returning any outcome to a caller.
    pub fn cluster_membership_read_v1(
        &self,
        request: ClusterMembershipReadRequestV1,
    ) -> Result<ClusterMembershipReadOutcomeV1, SdkError> {
        request
            .validate_v1()
            .map_err(|error| SdkError::Usage(error.to_string()))?;
        let mut response = self.cluster_membership_batch_read_v1(
            ClusterMembershipBatchReadRequestV1::single_v1(request),
        )?;
        response.outcomes.pop().ok_or_else(|| {
            SdkError::Protocol("validated single membership batch returned no outcome".to_string())
        })
    }

    /// Reads up to 16 `ClusterCard` memberships through one bounded transport
    /// request and rejects the entire call if any response entry is missing,
    /// reordered, duplicated, or stale.
    #[expect(
        clippy::needless_pass_by_value,
        reason = "taking the request by value binds the response-authority check to the exact request that was dispatched; a borrowed request could be mutated by the caller between dispatch and validate_against_v1"
    )]
    pub fn cluster_membership_batch_read_v1(
        &self,
        request: ClusterMembershipBatchReadRequestV1,
    ) -> Result<ClusterMembershipBatchReadResponseV1, SdkError> {
        request
            .validate_v1()
            .map_err(|error| SdkError::Usage(error.to_string()))?;
        let response = self.client.dispatch_query(
            quanta_index_contract::SearchPlaneQueryIpcRequest::ClusterMembershipRead(
                request.clone(),
            ),
        )?;
        match response {
            quanta_index_contract::SearchPlaneQueryIpcResponse::ClusterMembershipRead(batch) => {
                batch.validate_against_v1(&request).map_err(|failure| {
                    SdkError::Protocol(format!(
                        "cluster membership batch response failed request authority validation: {failure}"
                    ))
                })?;
                Ok(batch)
            }
            other @ (quanta_index_contract::SearchPlaneQueryIpcResponse::ActiveGenerationSnapshot(_) | quanta_index_contract::SearchPlaneQueryIpcResponse::ResolvedLexicalGeneration(_)
        | quanta_index_contract::SearchPlaneQueryIpcResponse::Text(_)
            | quanta_index_contract::SearchPlaneQueryIpcResponse::Symbol(_)
            | quanta_index_contract::SearchPlaneQueryIpcResponse::Semantic(_)
            | quanta_index_contract::SearchPlaneQueryIpcResponse::Hybrid(_)
            | quanta_index_contract::SearchPlaneQueryIpcResponse::HybridSeed(_)
            | quanta_index_contract::SearchPlaneQueryIpcResponse::History(_)
            | quanta_index_contract::SearchPlaneQueryIpcResponse::Structural(_)
            | quanta_index_contract::SearchPlaneQueryIpcResponse::RepoMapQuery(_)
            | quanta_index_contract::SearchPlaneQueryIpcResponse::Explain(_)
            | quanta_index_contract::SearchPlaneQueryIpcResponse::Error(_)
            | quanta_index_contract::SearchPlaneQueryIpcResponse::RuntimeMetadata(_)) => {
                Err(SdkError::unexpected_response(
                    "cluster membership read response",
                    QuantaIndex::query_response_kind(&other),
                ))
            }
        }
    }

    /// Is `candidate` in the generation's lexical index? An exact lookup;
    /// no score is traced because no query is named. A hybrid row's
    /// presence is that of its `candidate`.
    pub fn explain(
        &self,
        generation: GenerationPin,
        candidate: LexicalCandidate,
    ) -> Result<SearchPlaneExplainQueryResponse, SdkError> {
        self.explain_request(SearchPlaneExplainQueryRequest {
            generation,
            candidate: ExplainCandidateV1::Lexical(candidate),
            text_query: None,
            semantic_query_text: None,
        })
    }

    /// Why does `candidate` score what it scores under `text_query`
    /// (QI-BB-022)? The plane lowers the same plan the search ran and
    /// traces the lexical engine's score for exactly this candidate, as a
    /// lexical or semantic page carried it.
    pub fn explain_under_query(
        &self,
        generation: GenerationPin,
        candidate: LexicalCandidate,
        text_query: TextQueryRequest,
    ) -> Result<SearchPlaneExplainQueryResponse, SdkError> {
        self.explain_request(SearchPlaneExplainQueryRequest {
            generation,
            candidate: ExplainCandidateV1::Lexical(candidate),
            text_query: Some(text_query),
            semantic_query_text: None,
        })
    }

    /// Why does a hybrid `row` rank where it ranks (QI-BB-022)? The plane
    /// re-derives every lane against the index: the lexical lane under
    /// `text_query`'s plan, the dense lane by embedding `semantic_query_text`
    /// and scoring the row's stored vector exactly, and the fusion by
    /// re-running both bounded lanes and RRF. `text_query.top_k` must be the
    /// fused `top_k` the hybrid ran with. Each axis reports whether the
    /// carried provenance is what the index says now.
    pub fn explain_hybrid_under_queries(
        &self,
        generation: GenerationPin,
        row: HybridCandidateV1,
        text_query: TextQueryRequest,
        semantic_query_text: impl Into<String>,
    ) -> Result<SearchPlaneExplainQueryResponse, SdkError> {
        self.explain_request(SearchPlaneExplainQueryRequest {
            generation,
            candidate: ExplainCandidateV1::Hybrid(row),
            text_query: Some(text_query),
            semantic_query_text: Some(semantic_query_text.into()),
        })
    }

    fn explain_request(
        &self,
        request: SearchPlaneExplainQueryRequest,
    ) -> Result<SearchPlaneExplainQueryResponse, SdkError> {
        let response = self.client.dispatch_query(
            quanta_index_contract::SearchPlaneQueryIpcRequest::Explain(request),
        )?;
        match response {
            quanta_index_contract::SearchPlaneQueryIpcResponse::Explain(results) => Ok(results),
            other @ (quanta_index_contract::SearchPlaneQueryIpcResponse::ActiveGenerationSnapshot(_) | quanta_index_contract::SearchPlaneQueryIpcResponse::ResolvedLexicalGeneration(_)
        | quanta_index_contract::SearchPlaneQueryIpcResponse::Text(_)
            | quanta_index_contract::SearchPlaneQueryIpcResponse::Symbol(_)
            | quanta_index_contract::SearchPlaneQueryIpcResponse::Semantic(_)
            | quanta_index_contract::SearchPlaneQueryIpcResponse::Hybrid(_)
            | quanta_index_contract::SearchPlaneQueryIpcResponse::HybridSeed(_)
            | quanta_index_contract::SearchPlaneQueryIpcResponse::History(_)
            | quanta_index_contract::SearchPlaneQueryIpcResponse::Structural(_)
            | quanta_index_contract::SearchPlaneQueryIpcResponse::RepoMapQuery(_)
            | quanta_index_contract::SearchPlaneQueryIpcResponse::ClusterMembershipRead(
                _,
            )
            | quanta_index_contract::SearchPlaneQueryIpcResponse::Error(_)
            | quanta_index_contract::SearchPlaneQueryIpcResponse::RuntimeMetadata(_)) => {
                Err(SdkError::Protocol(format!(
                    "expected explain response, got {}",
                    QuantaIndex::query_response_kind(&other)
                )))
            }
        }
    }
}

pub struct HybridSeedQueryBuilder<
    'a,
    const HAS_TEXT: bool = false,
    const HAS_SEMANTIC_TEXT: bool = false,
    const HAS_SELECTION: bool = false,
    const HAS_TOP_K: bool = false,
> {
    client: &'a QuantaIndex,
    state: VectorQueryBuilderState,
}

impl<'a> HybridSeedQueryBuilder<'a> {
    fn new(client: &'a QuantaIndex) -> Self {
        Self {
            client,
            state: VectorQueryBuilderState::new(),
        }
    }
}

impl<
    'a,
    const HAS_TEXT: bool,
    const HAS_SEMANTIC_TEXT: bool,
    const HAS_SELECTION: bool,
    const HAS_TOP_K: bool,
> HybridSeedQueryBuilder<'a, HAS_TEXT, HAS_SEMANTIC_TEXT, HAS_SELECTION, HAS_TOP_K>
{
    fn transition<
        const NEXT_TEXT: bool,
        const NEXT_SEMANTIC_TEXT: bool,
        const NEXT_SELECTION: bool,
        const NEXT_TOP_K: bool,
    >(
        mut self,
        update: impl FnOnce(&mut VectorQueryBuilderState),
    ) -> HybridSeedQueryBuilder<'a, NEXT_TEXT, NEXT_SEMANTIC_TEXT, NEXT_SELECTION, NEXT_TOP_K> {
        update(&mut self.state);
        HybridSeedQueryBuilder {
            client: self.client,
            state: self.state,
        }
    }

    #[must_use]
    pub fn native(
        self,
        query_text: impl Into<String>,
    ) -> HybridSeedQueryBuilder<'a, true, HAS_SEMANTIC_TEXT, HAS_SELECTION, HAS_TOP_K> {
        self.transition(|state| {
            state.text_leg = Some((crate::TextQuerySyntax::Native, query_text.into()));
        })
    }

    #[must_use]
    pub fn sourcegraph(
        self,
        query_text: impl Into<String>,
    ) -> HybridSeedQueryBuilder<'a, true, HAS_SEMANTIC_TEXT, HAS_SELECTION, HAS_TOP_K> {
        self.transition(|state| {
            state.text_leg = Some((crate::TextQuerySyntax::Sourcegraph, query_text.into()));
        })
    }

    #[must_use]
    pub fn semantic_text(
        self,
        query_text: impl Into<String>,
    ) -> HybridSeedQueryBuilder<'a, HAS_TEXT, true, HAS_SELECTION, HAS_TOP_K> {
        self.transition(|state| {
            state.semantic_query_text = Some(query_text.into());
        })
    }

    /// Replace the canonical OR-set propagated to every sparse and dense leg.
    #[must_use]
    pub fn language_any_of(
        self,
        languages: impl IntoIterator<Item = quanta_index_contract::lex::LanguageCode>,
    ) -> Self {
        self.transition(|state| {
            state.constraints = std::mem::take(&mut state.constraints).with_languages(languages);
        })
    }

    /// Restrict every sparse and dense seed leg to one validated
    /// repository-relative path.
    #[must_use]
    pub fn exact_repo_relative_path(
        self,
        path: quanta_index_contract::ExactRepoRelativePathV1,
    ) -> Self {
        self.transition(|state| {
            state.constraints =
                std::mem::take(&mut state.constraints).with_exact_repo_relative_path(path);
        })
    }

    #[must_use]
    pub fn pinned(
        self,
        pin: GenerationPin,
    ) -> HybridSeedQueryBuilder<'a, HAS_TEXT, HAS_SEMANTIC_TEXT, true, HAS_TOP_K> {
        self.transition(|state| {
            state.selection = Some(GenerationSelector::Pinned(pin));
        })
    }

    #[must_use]
    pub fn active(
        self,
        repo_id: RepoId,
        revision_id: RevisionId,
    ) -> HybridSeedQueryBuilder<'a, HAS_TEXT, HAS_SEMANTIC_TEXT, true, HAS_TOP_K> {
        self.transition(|state| {
            state.selection = Some(GenerationSelector::Active {
                repo_id,
                revision_id,
            });
        })
    }

    #[must_use]
    pub fn top_k(
        self,
        top_k: u32,
    ) -> HybridSeedQueryBuilder<'a, HAS_TEXT, HAS_SEMANTIC_TEXT, HAS_SELECTION, true> {
        self.transition(|state| {
            state.top_k = Some(top_k);
        })
    }

    /// Add one independently ranked, storage-prefiltered semantic corpus lane.
    #[must_use]
    pub fn dense_corpus(mut self, corpus_kind: SemanticCorpusKindV1, top_k: u32) -> Self {
        self.state
            .dense_corpora
            .push(quanta_index_contract::SemanticSeedCorpusBudgetV1 { corpus_kind, top_k });
        self
    }
}

impl HybridSeedQueryBuilder<'_, true, true, true, true> {
    pub fn execute(self) -> Result<HybridSeedQueryResponse, SdkError> {
        dispatch_hybrid_seed_query_request_v1(self.client, self.state.build_hybrid_seed_request()?)
    }
}

fn dispatch_hybrid_seed_query_request_v1(
    client: &QuantaIndex,
    request: HybridSeedQueryRequest,
) -> Result<HybridSeedQueryResponse, SdkError> {
    let response = client
        .dispatch_query(quanta_index_contract::SearchPlaneQueryIpcRequest::HybridSeed(request))?;
    match response {
        quanta_index_contract::SearchPlaneQueryIpcResponse::HybridSeed(results) => Ok(results),
        other @ (quanta_index_contract::SearchPlaneQueryIpcResponse::ActiveGenerationSnapshot(
            _,
        )
        | quanta_index_contract::SearchPlaneQueryIpcResponse::ResolvedLexicalGeneration(
            _,
        )
        | quanta_index_contract::SearchPlaneQueryIpcResponse::Text(_)
        | quanta_index_contract::SearchPlaneQueryIpcResponse::Symbol(_)
        | quanta_index_contract::SearchPlaneQueryIpcResponse::Semantic(_)
        | quanta_index_contract::SearchPlaneQueryIpcResponse::Hybrid(_)
        | quanta_index_contract::SearchPlaneQueryIpcResponse::History(_)
        | quanta_index_contract::SearchPlaneQueryIpcResponse::Structural(_)
        | quanta_index_contract::SearchPlaneQueryIpcResponse::RepoMapQuery(_)
        | quanta_index_contract::SearchPlaneQueryIpcResponse::Explain(_)
        | quanta_index_contract::SearchPlaneQueryIpcResponse::ClusterMembershipRead(_)
        | quanta_index_contract::SearchPlaneQueryIpcResponse::Error(_)
        | quanta_index_contract::SearchPlaneQueryIpcResponse::RuntimeMetadata(_)) => {
            Err(SdkError::Protocol(format!(
                "expected hybrid seed query response, got {}",
                QuantaIndex::query_response_kind(&other)
            )))
        }
    }
}

/// Type-state builder for the hybrid route (QI-BB-018).
///
/// A text lane, a semantic text, a generation selection and a fused `top_k`
/// are all required before `execute` exists. There is no `after`: the hybrid
/// route serves one fused page and does not paginate.
pub struct HybridQueryBuilder<
    'a,
    const HAS_TEXT: bool = false,
    const HAS_SEMANTIC_TEXT: bool = false,
    const HAS_SELECTION: bool = false,
    const HAS_TOP_K: bool = false,
> {
    client: &'a QuantaIndex,
    state: VectorQueryBuilderState,
}

impl<'a> HybridQueryBuilder<'a> {
    fn new(client: &'a QuantaIndex) -> Self {
        Self {
            client,
            state: VectorQueryBuilderState::new(),
        }
    }
}

impl<
    'a,
    const HAS_TEXT: bool,
    const HAS_SEMANTIC_TEXT: bool,
    const HAS_SELECTION: bool,
    const HAS_TOP_K: bool,
> HybridQueryBuilder<'a, HAS_TEXT, HAS_SEMANTIC_TEXT, HAS_SELECTION, HAS_TOP_K>
{
    fn transition<
        const NEXT_TEXT: bool,
        const NEXT_SEMANTIC_TEXT: bool,
        const NEXT_SELECTION: bool,
        const NEXT_TOP_K: bool,
    >(
        mut self,
        update: impl FnOnce(&mut VectorQueryBuilderState),
    ) -> HybridQueryBuilder<'a, NEXT_TEXT, NEXT_SEMANTIC_TEXT, NEXT_SELECTION, NEXT_TOP_K> {
        update(&mut self.state);
        HybridQueryBuilder {
            client: self.client,
            state: self.state,
        }
    }

    /// The lexical lane's query, in native syntax.
    #[must_use]
    pub fn native(
        self,
        query_text: impl Into<String>,
    ) -> HybridQueryBuilder<'a, true, HAS_SEMANTIC_TEXT, HAS_SELECTION, HAS_TOP_K> {
        self.transition(|state| {
            state.text_leg = Some((crate::TextQuerySyntax::Native, query_text.into()));
        })
    }

    /// The lexical lane's query, in Sourcegraph syntax.
    #[must_use]
    pub fn sourcegraph(
        self,
        query_text: impl Into<String>,
    ) -> HybridQueryBuilder<'a, true, HAS_SEMANTIC_TEXT, HAS_SELECTION, HAS_TOP_K> {
        self.transition(|state| {
            state.text_leg = Some((crate::TextQuerySyntax::Sourcegraph, query_text.into()));
        })
    }

    /// The dense lane's query text, embedded by the search plane.
    #[must_use]
    pub fn semantic_text(
        self,
        query_text: impl Into<String>,
    ) -> HybridQueryBuilder<'a, HAS_TEXT, true, HAS_SELECTION, HAS_TOP_K> {
        self.transition(|state| {
            state.semantic_query_text = Some(query_text.into());
        })
    }

    /// Replace the canonical OR-set pushed down to both lanes.
    #[must_use]
    pub fn language_any_of(
        self,
        languages: impl IntoIterator<Item = quanta_index_contract::lex::LanguageCode>,
    ) -> Self {
        self.transition(|state| {
            state.constraints = std::mem::take(&mut state.constraints).with_languages(languages);
        })
    }

    /// Restrict both lanes to one validated repository-relative path.
    #[must_use]
    pub fn exact_repo_relative_path(
        self,
        path: quanta_index_contract::ExactRepoRelativePathV1,
    ) -> Self {
        self.transition(|state| {
            state.constraints =
                std::mem::take(&mut state.constraints).with_exact_repo_relative_path(path);
        })
    }

    #[must_use]
    pub fn pinned(
        self,
        pin: GenerationPin,
    ) -> HybridQueryBuilder<'a, HAS_TEXT, HAS_SEMANTIC_TEXT, true, HAS_TOP_K> {
        self.transition(|state| {
            state.selection = Some(GenerationSelector::Pinned(pin));
        })
    }

    #[must_use]
    pub fn active(
        self,
        repo_id: RepoId,
        revision_id: RevisionId,
    ) -> HybridQueryBuilder<'a, HAS_TEXT, HAS_SEMANTIC_TEXT, true, HAS_TOP_K> {
        self.transition(|state| {
            state.selection = Some(GenerationSelector::Active {
                repo_id,
                revision_id,
            });
        })
    }

    /// The fused page's cap; each lane is over-fetched internally.
    #[must_use]
    pub fn top_k(
        self,
        top_k: u32,
    ) -> HybridQueryBuilder<'a, HAS_TEXT, HAS_SEMANTIC_TEXT, HAS_SELECTION, true> {
        self.transition(|state| {
            state.top_k = Some(top_k);
        })
    }
}

impl HybridQueryBuilder<'_, true, true, true, true> {
    pub fn execute(self) -> Result<HybridQueryResponse, SdkError> {
        dispatch_hybrid_query_request_v1(self.client, self.state.build_hybrid_request()?)
    }
}

fn dispatch_hybrid_query_request_v1(
    client: &QuantaIndex,
    request: HybridQueryRequest,
) -> Result<HybridQueryResponse, SdkError> {
    let response = client.dispatch_query(
        quanta_index_contract::SearchPlaneQueryIpcRequest::Hybrid(request),
    )?;
    match response {
        quanta_index_contract::SearchPlaneQueryIpcResponse::Hybrid(results) => Ok(results),
        other @ (quanta_index_contract::SearchPlaneQueryIpcResponse::ActiveGenerationSnapshot(
            _,
        )
        | quanta_index_contract::SearchPlaneQueryIpcResponse::ResolvedLexicalGeneration(
            _,
        )
        | quanta_index_contract::SearchPlaneQueryIpcResponse::Text(_)
        | quanta_index_contract::SearchPlaneQueryIpcResponse::Symbol(_)
        | quanta_index_contract::SearchPlaneQueryIpcResponse::Semantic(_)
        | quanta_index_contract::SearchPlaneQueryIpcResponse::HybridSeed(_)
        | quanta_index_contract::SearchPlaneQueryIpcResponse::History(_)
        | quanta_index_contract::SearchPlaneQueryIpcResponse::Structural(_)
        | quanta_index_contract::SearchPlaneQueryIpcResponse::RepoMapQuery(_)
        | quanta_index_contract::SearchPlaneQueryIpcResponse::Explain(_)
        | quanta_index_contract::SearchPlaneQueryIpcResponse::ClusterMembershipRead(_)
        | quanta_index_contract::SearchPlaneQueryIpcResponse::Error(_)
        | quanta_index_contract::SearchPlaneQueryIpcResponse::RuntimeMetadata(_)) => {
            Err(SdkError::Protocol(format!(
                "expected hybrid query response, got {}",
                QuantaIndex::query_response_kind(&other)
            )))
        }
    }
}
