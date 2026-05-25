use quanta_index_contract::{
    GenerationSelector, RepoId, RevisionId, SymbolQueryResponse, TextQuerySyntax,
};

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
}

pub struct SymbolQueryBuilder<'a> {
    client: &'a QuantaIndex,
    syntax: TextQuerySyntax,
    query_text: Option<String>,
    selection: Option<GenerationSelector>,
    top_k: Option<u32>,
}

impl<'a> SymbolQueryBuilder<'a> {
    const fn new(client: &'a QuantaIndex) -> Self {
        Self {
            client,
            syntax: TextQuerySyntax::Native,
            query_text: None,
            selection: None,
            top_k: None,
        }
    }

    #[must_use]
    pub fn native(mut self, query_text: impl Into<String>) -> Self {
        self.syntax = TextQuerySyntax::Native;
        self.query_text = Some(query_text.into());
        self
    }

    #[must_use]
    pub fn sourcegraph(mut self, query_text: impl Into<String>) -> Self {
        self.syntax = TextQuerySyntax::Sourcegraph;
        self.query_text = Some(query_text.into());
        self
    }

    #[must_use]
    pub fn pinned(mut self, pin: quanta_index_contract::GenerationPin) -> Self {
        self.selection = Some(GenerationSelector::Pinned(pin));
        self
    }

    #[must_use]
    pub fn active(mut self, repo_id: RepoId, revision_id: RevisionId) -> Self {
        self.selection = Some(GenerationSelector::Active {
            repo_id,
            revision_id,
        });
        self
    }

    /// QI-QRY-01: required result cap.
    #[must_use]
    pub fn top_k(mut self, top_k: u32) -> Self {
        self.top_k = Some(top_k);
        self
    }

    pub fn execute(self) -> Result<SymbolQueryResponse, SdkError> {
        let query_text = self
            .query_text
            .ok_or_else(|| SdkError::Usage("symbol query text is required".to_string()))?;
        let selection = self.selection.ok_or_else(|| {
            SdkError::Usage("symbol generation selection is required".to_string())
        })?;
        let top_k = self
            .top_k
            .ok_or_else(|| SdkError::Usage("symbol top_k is required".to_string()))?;
        let (generation, generation_selector) = QuantaIndex::selection_to_fields(selection);
        let response = self.client.dispatch_query(
            quanta_index_contract::SearchPlaneQueryIpcRequest::Symbol(
                quanta_index_contract::SymbolQueryRequest {
                    syntax: self.syntax,
                    query_text,
                    generation,
                    generation_selector,
                    top_k,
                },
            ),
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
            | quanta_index_contract::SearchPlaneQueryIpcResponse::Sourcegraph(_)) => {
                Err(SdkError::Protocol(format!(
                    "expected symbol query response, got {}",
                    QuantaIndex::query_response_kind(&other)
                )))
            }
        }
    }
}
