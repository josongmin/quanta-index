use quanta_index_contract::{
    GenerationPin, GenerationSelector, HybridSeedQueryRequest, HybridSeedQueryResponse,
    LexicalCandidate, RepoId, RevisionId, SearchPlaneExplainQueryRequest,
    SearchPlaneExplainQueryResponse, SemanticCorpusKindV1,
};

use crate::{QuantaIndex, SdkError, text_query_builder::VectorQueryBuilderState};

pub struct SearchNamespace<'a> {
    client: &'a QuantaIndex,
}

impl<'a> SearchNamespace<'a> {
    pub(super) const fn new(client: &'a QuantaIndex) -> Self {
        Self { client }
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

    pub fn explain(
        &self,
        generation: GenerationPin,
        candidate: LexicalCandidate,
    ) -> Result<SearchPlaneExplainQueryResponse, SdkError> {
        let response = self.client.dispatch_query(
            quanta_index_contract::SearchPlaneQueryIpcRequest::Explain(
                SearchPlaneExplainQueryRequest {
                    generation,
                    candidate,
                },
            ),
        )?;
        match response {
            quanta_index_contract::SearchPlaneQueryIpcResponse::Explain(results) => Ok(results),
            other @ (quanta_index_contract::SearchPlaneQueryIpcResponse::Text(_)
            | quanta_index_contract::SearchPlaneQueryIpcResponse::Symbol(_)
            | quanta_index_contract::SearchPlaneQueryIpcResponse::Semantic(_)
            | quanta_index_contract::SearchPlaneQueryIpcResponse::Hybrid(_)
            | quanta_index_contract::SearchPlaneQueryIpcResponse::HybridSeed(_)
            | quanta_index_contract::SearchPlaneQueryIpcResponse::History(_)
            | quanta_index_contract::SearchPlaneQueryIpcResponse::Structural(_)
            | quanta_index_contract::SearchPlaneQueryIpcResponse::RepoMapQuery(_)
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
    const fn new(client: &'a QuantaIndex) -> Self {
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
        other @ (quanta_index_contract::SearchPlaneQueryIpcResponse::Text(_)
        | quanta_index_contract::SearchPlaneQueryIpcResponse::Symbol(_)
        | quanta_index_contract::SearchPlaneQueryIpcResponse::Semantic(_)
        | quanta_index_contract::SearchPlaneQueryIpcResponse::Hybrid(_)
        | quanta_index_contract::SearchPlaneQueryIpcResponse::History(_)
        | quanta_index_contract::SearchPlaneQueryIpcResponse::Structural(_)
        | quanta_index_contract::SearchPlaneQueryIpcResponse::RepoMapQuery(_)
        | quanta_index_contract::SearchPlaneQueryIpcResponse::Explain(_)
        | quanta_index_contract::SearchPlaneQueryIpcResponse::Error(_)
        | quanta_index_contract::SearchPlaneQueryIpcResponse::RuntimeMetadata(_)) => {
            Err(SdkError::Protocol(format!(
                "expected hybrid seed query response, got {}",
                QuantaIndex::query_response_kind(&other)
            )))
        }
    }
}
