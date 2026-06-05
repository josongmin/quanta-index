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

pub struct SymbolQueryBuilder<
    'a,
    const HAS_TEXT: bool = false,
    const HAS_SELECTION: bool = false,
    const HAS_TOP_K: bool = false,
> {
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
}

impl<'a, const HAS_TEXT: bool, const HAS_SELECTION: bool, const HAS_TOP_K: bool>
    SymbolQueryBuilder<'a, HAS_TEXT, HAS_SELECTION, HAS_TOP_K>
{
    fn transition<const NEXT_TEXT: bool, const NEXT_SELECTION: bool, const NEXT_TOP_K: bool>(
        mut self,
        update: impl FnOnce(&mut TextQueryBuilderState),
    ) -> SymbolQueryBuilder<'a, NEXT_TEXT, NEXT_SELECTION, NEXT_TOP_K> {
        update(&mut self.state);
        SymbolQueryBuilder {
            client: self.client,
            state: self.state,
        }
    }

    #[must_use]
    pub fn native(
        self,
        query_text: impl Into<String>,
    ) -> SymbolQueryBuilder<'a, true, HAS_SELECTION, HAS_TOP_K> {
        self.transition(|state| {
            state.syntax = TextQuerySyntax::Native;
            state.query_text = Some(query_text.into());
        })
    }

    #[must_use]
    pub fn sourcegraph(
        self,
        query_text: impl Into<String>,
    ) -> SymbolQueryBuilder<'a, true, HAS_SELECTION, HAS_TOP_K> {
        self.transition(|state| {
            state.syntax = TextQuerySyntax::Sourcegraph;
            state.query_text = Some(query_text.into());
        })
    }

    #[must_use]
    pub fn pinned(
        self,
        pin: quanta_index_contract::GenerationPin,
    ) -> SymbolQueryBuilder<'a, HAS_TEXT, true, HAS_TOP_K> {
        self.transition(|state| {
            state.selection = Some(GenerationSelector::Pinned(pin));
        })
    }

    #[must_use]
    pub fn active(
        self,
        repo_id: RepoId,
        revision_id: RevisionId,
    ) -> SymbolQueryBuilder<'a, HAS_TEXT, true, HAS_TOP_K> {
        self.transition(|state| {
            state.selection = Some(GenerationSelector::Active {
                repo_id,
                revision_id,
            });
        })
    }

    /// QI-QRY-01: required result cap.
    #[must_use]
    pub fn top_k(self, top_k: u32) -> SymbolQueryBuilder<'a, HAS_TEXT, HAS_SELECTION, true> {
        self.transition(|state| {
            state.top_k = Some(top_k);
        })
    }
}

impl SymbolQueryBuilder<'_, true, true, true> {
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
        | quanta_index_contract::SearchPlaneQueryIpcResponse::HybridSeed(_)
        | quanta_index_contract::SearchPlaneQueryIpcResponse::History(_)
        | quanta_index_contract::SearchPlaneQueryIpcResponse::Structural(_)
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
