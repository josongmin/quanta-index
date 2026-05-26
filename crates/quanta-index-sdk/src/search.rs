use quanta_index_contract::{
    GenerationPin, GenerationSelector, HybridQueryRequest, HybridQueryResponse, LexicalCandidate,
    RepoId, RevisionId, SearchPlaneExplainQueryRequest, SearchPlaneExplainQueryResponse,
};

use crate::{QuantaIndex, SdkError, SemanticVector, text_query_builder::VectorQueryBuilderState};

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
            | quanta_index_contract::SearchPlaneQueryIpcResponse::Bridge(_)
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

pub struct HybridQueryBuilder<'a> {
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

    #[must_use]
    pub fn native(mut self, query_text: impl Into<String>) -> Self {
        self.state.text_leg = Some((crate::TextQuerySyntax::Native, query_text.into()));
        self
    }

    #[must_use]
    pub fn sourcegraph(mut self, query_text: impl Into<String>) -> Self {
        self.state.text_leg = Some((crate::TextQuerySyntax::Sourcegraph, query_text.into()));
        self
    }

    #[must_use]
    pub fn vector(mut self, vector: Vec<f32>) -> Self {
        self.state.vector = Some(SemanticVector::Inline(vector));
        self
    }

    #[must_use]
    pub fn vector_handle(mut self, handle: impl Into<String>) -> Self {
        self.state.vector = Some(SemanticVector::Handle(handle.into()));
        self
    }

    #[must_use]
    pub fn pinned(mut self, pin: GenerationPin) -> Self {
        self.state.selection = Some(GenerationSelector::Pinned(pin));
        self
    }

    #[must_use]
    pub fn active(mut self, repo_id: RepoId, revision_id: RevisionId) -> Self {
        self.state.selection = Some(GenerationSelector::Active {
            repo_id,
            revision_id,
        });
        self
    }

    #[must_use]
    pub fn top_k(mut self, top_k: u32) -> Self {
        self.state.top_k = Some(top_k);
        self
    }

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
        | quanta_index_contract::SearchPlaneQueryIpcResponse::Bridge(_)
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
