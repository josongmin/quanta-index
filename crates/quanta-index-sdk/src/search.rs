use quanta_index_contract::{
    GenerationPin, GenerationSelector, HybridQueryRequest, HybridQueryResponse, LexicalCandidate,
    RepoId, RevisionId, SearchPlaneExplainQueryRequest, SearchPlaneExplainQueryResponse,
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
    pub fn hybrid(&self) -> HybridQueryBuilder<'a> {
        HybridQueryBuilder::new(self.client)
    }

    /// Contract-exact hybrid query replay surface. Accepts the shared wire
    /// DTO unchanged and routes it through the query transport.
    pub fn hybrid_request(
        &self,
        request: HybridQueryRequest,
    ) -> Result<HybridQueryResponse, SdkError> {
        dispatch_hybrid_query_request_v1(self.client, request)
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

    #[must_use]
    pub fn native(
        self,
        query_text: impl Into<String>,
    ) -> HybridQueryBuilder<'a, true, HAS_SEMANTIC_TEXT, HAS_SELECTION, HAS_TOP_K> {
        self.transition(|state| {
            state.text_leg = Some((crate::TextQuerySyntax::Native, query_text.into()));
        })
    }

    #[must_use]
    pub fn sourcegraph(
        self,
        query_text: impl Into<String>,
    ) -> HybridQueryBuilder<'a, true, HAS_SEMANTIC_TEXT, HAS_SELECTION, HAS_TOP_K> {
        self.transition(|state| {
            state.text_leg = Some((crate::TextQuerySyntax::Sourcegraph, query_text.into()));
        })
    }

    #[must_use]
    pub fn semantic_text(
        self,
        query_text: impl Into<String>,
    ) -> HybridQueryBuilder<'a, HAS_TEXT, true, HAS_SELECTION, HAS_TOP_K> {
        self.transition(|state| {
            state.semantic_query_text = Some(query_text.into());
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
        other @ (quanta_index_contract::SearchPlaneQueryIpcResponse::Text(_)
        | quanta_index_contract::SearchPlaneQueryIpcResponse::Symbol(_)
        | quanta_index_contract::SearchPlaneQueryIpcResponse::Semantic(_)
        | quanta_index_contract::SearchPlaneQueryIpcResponse::History(_)
        | quanta_index_contract::SearchPlaneQueryIpcResponse::Structural(_)
        | quanta_index_contract::SearchPlaneQueryIpcResponse::RepoMapQuery(_)
        | quanta_index_contract::SearchPlaneQueryIpcResponse::Explain(_)
        | quanta_index_contract::SearchPlaneQueryIpcResponse::Error(_)
        | quanta_index_contract::SearchPlaneQueryIpcResponse::RuntimeMetadata(_)) => {
            Err(SdkError::Protocol(format!(
                "expected hybrid query response, got {}",
                QuantaIndex::query_response_kind(&other)
            )))
        }
    }
}
