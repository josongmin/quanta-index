use quanta_index_contract::{
    GenerationSelector, RepoId, RevisionId, SymbolQueryResponse, TextQuerySyntax,
};

use crate::text_query_builder::TextQueryBuilderState;
use crate::{QuantaIndex, SdkError};

pub struct SymbolNamespace<'a> {
    client: &'a QuantaIndex,
}

impl<'a> SymbolNamespace<'a> {
    pub(super) const fn new(client: &'a QuantaIndex) -> Self {
        Self { client }
    }

    #[must_use]
    pub fn query(&self) -> SymbolQueryBuilder<'a> {
        SymbolQueryBuilder::new(self.client)
    }

    /// Contract-exact query replay surface. Accepts the shared wire DTO
    /// unchanged and routes it through the query transport.
    pub fn query_request(
        &self,
        request: quanta_index_contract::SymbolQueryRequest,
    ) -> Result<SymbolQueryResponse, SdkError> {
        dispatch_symbol_query_request_v1(self.client, request)
    }
}

pub struct SymbolQueryBuilder<'a> {
    client: &'a QuantaIndex,
    state: TextQueryBuilderState,
}

impl<'a> SymbolQueryBuilder<'a> {
    const fn new(client: &'a QuantaIndex) -> Self {
        Self {
            client,
            state: TextQueryBuilderState::new(),
        }
    }

    #[must_use]
    pub fn native(mut self, query_text: impl Into<String>) -> Self {
        self.state.syntax = TextQuerySyntax::Native;
        self.state.query_text = Some(query_text.into());
        self
    }

    #[must_use]
    pub fn sourcegraph(mut self, query_text: impl Into<String>) -> Self {
        self.state.syntax = TextQuerySyntax::Sourcegraph;
        self.state.query_text = Some(query_text.into());
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

    /// QI-QRY-01: required result cap.
    #[must_use]
    pub fn top_k(mut self, top_k: u32) -> Self {
        self.state.top_k = Some(top_k);
        self
    }

    pub fn execute(self) -> Result<SymbolQueryResponse, SdkError> {
        let request = self.state.build_request("symbol")?;
        dispatch_symbol_query_request_v1(
            self.client,
            quanta_index_contract::SymbolQueryRequest {
                syntax: request.syntax,
                query_text: request.query_text,
                generation: request.generation,
                generation_selector: request.generation_selector,
                top_k: request.top_k,
            },
        )
    }
}

fn dispatch_symbol_query_request_v1(
    client: &QuantaIndex,
    request: quanta_index_contract::SymbolQueryRequest,
) -> Result<SymbolQueryResponse, SdkError> {
    let response = client.dispatch_query(
        quanta_index_contract::SearchPlaneQueryIpcRequest::Symbol(request),
    )?;
    match response {
        quanta_index_contract::SearchPlaneQueryIpcResponse::Symbol(results) => Ok(results),
        other @ (quanta_index_contract::SearchPlaneQueryIpcResponse::Text(_)
        | quanta_index_contract::SearchPlaneQueryIpcResponse::Semantic(_)
        | quanta_index_contract::SearchPlaneQueryIpcResponse::Hybrid(_)
        | quanta_index_contract::SearchPlaneQueryIpcResponse::History(_)
        | quanta_index_contract::SearchPlaneQueryIpcResponse::Structural(_)
        | quanta_index_contract::SearchPlaneQueryIpcResponse::Bridge(_)
        | quanta_index_contract::SearchPlaneQueryIpcResponse::RepoMapQuery(_)
        | quanta_index_contract::SearchPlaneQueryIpcResponse::Explain(_)
        | quanta_index_contract::SearchPlaneQueryIpcResponse::Error(_)
        | quanta_index_contract::SearchPlaneQueryIpcResponse::RuntimeMetadata(_)) => {
            Err(SdkError::Protocol(format!(
                "expected symbol query response, got {}",
                QuantaIndex::query_response_kind(&other)
            )))
        }
    }
}
