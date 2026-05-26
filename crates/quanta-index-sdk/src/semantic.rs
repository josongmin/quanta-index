use quanta_index_contract::{
    GenerationSelector, RepoId, RevisionId, SemanticQueryRequest, SemanticQueryResponse,
};

use crate::{QuantaIndex, SdkError, TextQuerySyntax, text_query_builder::VectorQueryBuilderState};

pub struct SemanticNamespace<'a> {
    client: &'a QuantaIndex,
}

impl<'a> SemanticNamespace<'a> {
    pub(super) const fn new(client: &'a QuantaIndex) -> Self {
        Self { client }
    }

    /// Public semantic surface is query-only. Corpus authority is derived
    /// inside `searchd` from lexical ingest; SDK callers do not publish
    /// semantic batches directly.
    #[must_use]
    pub fn query(&self) -> SemanticQueryBuilder<'a> {
        <SemanticNs as crate::NamespaceQuery>::query(self.client)
    }

    /// Contract-exact query replay surface. Accepts the shared wire DTO
    /// unchanged and routes it through the query transport.
    pub fn query_request(
        &self,
        request: SemanticQueryRequest,
    ) -> Result<SemanticQueryResponse, SdkError> {
        dispatch_semantic_query_request_v1(self.client, request)
    }
}

/// QI-NS-01: marker type for the built-in semantic namespace.
struct SemanticNs;

impl crate::NamespaceQuery for SemanticNs {
    type QueryBuilder<'a> = SemanticQueryBuilder<'a>;

    fn query(client: &QuantaIndex) -> SemanticQueryBuilder<'_> {
        SemanticQueryBuilder::new(client)
    }
}

pub struct SemanticQueryBuilder<'a> {
    client: &'a QuantaIndex,
    state: VectorQueryBuilderState,
}

impl<'a> SemanticQueryBuilder<'a> {
    const fn new(client: &'a QuantaIndex) -> Self {
        Self {
            client,
            state: VectorQueryBuilderState::new(),
        }
    }

    #[must_use]
    pub fn text(mut self, query_text: impl Into<String>) -> Self {
        self.state.semantic_query_text = Some(query_text.into());
        self
    }

    #[must_use]
    pub fn scope_native(mut self, query_text: impl Into<String>) -> Self {
        self.state.scope_leg = Some((TextQuerySyntax::Native, query_text.into()));
        self
    }

    #[must_use]
    pub fn scope_sourcegraph(mut self, query_text: impl Into<String>) -> Self {
        self.state.scope_leg = Some((TextQuerySyntax::Sourcegraph, query_text.into()));
        self
    }

    #[must_use]
    pub fn pinned(mut self, pin: quanta_index_contract::GenerationPin) -> Self {
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

    /// QI-QRY-01: explicit lexical-scope candidate cap. When the
    /// builder's lexical scope (`scope_native` / `scope_sourcegraph`) is
    /// set, this MUST also be set; [`Self::execute`] returns
    /// [`SdkError::Usage`] otherwise. The two values are semantically
    /// distinct: outer `top_k` is the final semantic recall cap, while
    /// `scope_top_k` is the lexical candidate cap fed into the hybrid
    /// scope stage.
    #[must_use]
    pub fn scope_top_k(mut self, scope_top_k: u32) -> Self {
        self.state.scope_top_k = Some(scope_top_k);
        self
    }

    pub fn execute(self) -> Result<SemanticQueryResponse, SdkError> {
        dispatch_semantic_query_request_v1(self.client, self.state.build_semantic_request()?)
    }
}

fn dispatch_semantic_query_request_v1(
    client: &QuantaIndex,
    request: SemanticQueryRequest,
) -> Result<SemanticQueryResponse, SdkError> {
    let response = client.dispatch_query(
        quanta_index_contract::SearchPlaneQueryIpcRequest::Semantic(request),
    )?;
    match response {
        quanta_index_contract::SearchPlaneQueryIpcResponse::Semantic(results) => Ok(results),
        other @ (quanta_index_contract::SearchPlaneQueryIpcResponse::Text(_)
        | quanta_index_contract::SearchPlaneQueryIpcResponse::Symbol(_)
        | quanta_index_contract::SearchPlaneQueryIpcResponse::Hybrid(_)
        | quanta_index_contract::SearchPlaneQueryIpcResponse::History(_)
        | quanta_index_contract::SearchPlaneQueryIpcResponse::Structural(_)
        | quanta_index_contract::SearchPlaneQueryIpcResponse::Bridge(_)
        | quanta_index_contract::SearchPlaneQueryIpcResponse::RepoMapQuery(_)
        | quanta_index_contract::SearchPlaneQueryIpcResponse::Explain(_)
        | quanta_index_contract::SearchPlaneQueryIpcResponse::Error(_)
        | quanta_index_contract::SearchPlaneQueryIpcResponse::RuntimeMetadata(_)) => {
            Err(SdkError::Protocol(format!(
                "expected semantic response, got {}",
                QuantaIndex::query_response_kind(&other)
            )))
        }
    }
}
