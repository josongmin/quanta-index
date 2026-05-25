use quanta_index_contract::{
    GenerationPin, SearchPlaneQueryIpcRequest, SearchPlaneQueryIpcResponse,
    SearchPlaneSourcegraphQueryRequest, SearchPlaneSourcegraphQueryResponse,
};

use crate::{QuantaIndex, SdkError};

pub struct SourcegraphNamespace<'a> {
    client: &'a QuantaIndex,
}

impl<'a> SourcegraphNamespace<'a> {
    pub(super) const fn new(client: &'a QuantaIndex) -> Self {
        Self { client }
    }

    #[must_use]
    pub fn query(&self) -> SourcegraphQueryBuilder<'a> {
        SourcegraphQueryBuilder::new(self.client)
    }
}

pub struct SourcegraphQueryBuilder<'a> {
    client: &'a QuantaIndex,
    source_syntax: Option<String>,
    sg_version: Option<String>,
    generation: Option<GenerationPin>,
    top_k: Option<u32>,
}

impl<'a> SourcegraphQueryBuilder<'a> {
    const fn new(client: &'a QuantaIndex) -> Self {
        Self {
            client,
            source_syntax: None,
            sg_version: None,
            generation: None,
            top_k: None,
        }
    }

    #[must_use]
    pub fn source_syntax(mut self, source_syntax: impl Into<String>) -> Self {
        self.source_syntax = Some(source_syntax.into());
        self
    }

    #[must_use]
    pub fn sg_version(mut self, sg_version: impl Into<String>) -> Self {
        self.sg_version = Some(sg_version.into());
        self
    }

    #[must_use]
    pub fn pinned(mut self, generation: GenerationPin) -> Self {
        self.generation = Some(generation);
        self
    }

    #[must_use]
    pub fn top_k(mut self, top_k: u32) -> Self {
        self.top_k = Some(top_k);
        self
    }

    pub fn execute(self) -> Result<SearchPlaneSourcegraphQueryResponse, SdkError> {
        let source_syntax = self
            .source_syntax
            .ok_or_else(|| SdkError::Usage("sourcegraph query text is required".to_string()))?;
        let sg_version = self
            .sg_version
            .ok_or_else(|| SdkError::Usage("sourcegraph sg_version is required".to_string()))?;
        let generation = self
            .generation
            .ok_or_else(|| SdkError::Usage("sourcegraph generation pin is required".to_string()))?;
        let top_k = self
            .top_k
            .ok_or_else(|| SdkError::Usage("sourcegraph top_k is required".to_string()))?;
        let response = self
            .client
            .dispatch_query(SearchPlaneQueryIpcRequest::Sourcegraph(
                SearchPlaneSourcegraphQueryRequest {
                    source_syntax: source_syntax.into_boxed_str(),
                    sg_version: sg_version.into_boxed_str(),
                    generation: Some(generation),
                    top_k,
                },
            ))?;
        match response {
            SearchPlaneQueryIpcResponse::Sourcegraph(results) => Ok(results),
            other @ (SearchPlaneQueryIpcResponse::Text(_)
            | SearchPlaneQueryIpcResponse::Symbol(_)
            | SearchPlaneQueryIpcResponse::Semantic(_)
            | SearchPlaneQueryIpcResponse::Hybrid(_)
            | SearchPlaneQueryIpcResponse::History(_)
            | SearchPlaneQueryIpcResponse::Structural(_)
            | SearchPlaneQueryIpcResponse::Bridge(_)
            | SearchPlaneQueryIpcResponse::RepoMapQuery(_)
            | SearchPlaneQueryIpcResponse::Explain(_)
            | SearchPlaneQueryIpcResponse::Error(_)) => Err(SdkError::Protocol(format!(
                "expected sourcegraph query response, got {}",
                QuantaIndex::query_response_kind(&other)
            ))),
        }
    }
}
